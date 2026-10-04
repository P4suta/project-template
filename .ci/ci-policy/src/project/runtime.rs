use std::{
    collections::{BTreeMap, BTreeSet},
    ffi::OsString,
    fs,
    io::Read,
    path::Path,
    process::{Command, Stdio},
    thread,
    time::{Duration, Instant},
};

use anyhow::{Context, Result, ensure};
use process_wrap::std::{ChildWrapper, CommandWrap};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use yaml_rust2::{Yaml, YamlLoader};

use super::protocol::{
    Budget, Cache, Completion, Coverage, Decision, Phase, Progress, budget_valid, decide, progress,
};

const CONTRACT: &str = ".ci/verification.json";
const GIT_LOCATION: [&str; 6] = [
    "GIT_DIR",
    "GIT_WORK_TREE",
    "GIT_INDEX_FILE",
    "GIT_COMMON_DIR",
    "GIT_OBJECT_DIRECTORY",
    "GIT_ALTERNATE_OBJECT_DIRECTORIES",
];

#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct Check {
    id: String,
    phase: Phase,
    platforms: Vec<String>,
    command: Vec<String>,
    tools: Vec<String>,
    inputs: Vec<String>,
    environment: Vec<String>,
    cache: Cache,
    timeout_seconds: u64,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Ci {
    workflow: String,
    job: String,
    checks: Vec<String>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Contract {
    version: u8,
    checks: Vec<Check>,
    ci: Vec<Ci>,
    #[serde(default)]
    hosted: Vec<Hosted>,
}

#[derive(Deserialize)]
#[serde(rename_all = "snake_case")]
enum Capability {
    Security,
    Policy,
    Aggregate,
    Approval,
    Campaign,
    Initialization,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Hosted {
    workflow: String,
    job: String,
    capability: Capability,
}

#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct Success {
    check: String,
    identity: String,
}

#[derive(Clone, Copy)]
pub enum Scope<'a> {
    Working,
    Index,
    Revision(Revision<'a>),
}

#[derive(Clone, Copy)]
pub struct Revision<'a>(&'a str);

impl<'a> Revision<'a> {
    pub fn parse(value: &'a str) -> Result<Self> {
        ensure!(
            crate::exact_hash(value.as_bytes(), 40),
            "project revision must be exact"
        );
        Ok(Self(value))
    }
}

#[derive(Clone, PartialEq, Eq)]
struct Input {
    bytes: Vec<u8>,
    executable: bool,
}

type Files = BTreeMap<String, Input>;

fn literal(value: &str) -> bool {
    !value.is_empty()
        && value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || b"._-".contains(&byte))
}

fn private_command(program: &str, root: &Path) -> Command {
    let mut command = Command::new(program);
    command.current_dir(root);
    for name in GIT_LOCATION {
        command.env_remove(name);
    }
    command
}

fn git(root: &Path, arguments: &[&str], private: bool) -> Result<Vec<u8>> {
    let mut command = if private {
        private_command("git", root)
    } else {
        let mut command = Command::new("git");
        command.current_dir(root);
        command
    };
    let output = command
        .args(["-c", "core.fsmonitor=false"])
        .args(arguments)
        .output()?;
    ensure!(
        output.status.success(),
        "cannot establish project source: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    Ok(output.stdout)
}

fn bounded(path: &Path, limit: u64) -> Result<Vec<u8>> {
    let metadata = fs::symlink_metadata(path)?;
    ensure!(
        metadata.file_type().is_file() && metadata.len() <= limit,
        "project input must be a bounded regular file"
    );
    let mut bytes = Vec::new();
    fs::File::open(path)?
        .take(limit + 1)
        .read_to_end(&mut bytes)?;
    ensure!(
        bytes.len() as u64 <= limit,
        "project input exceeded its size budget"
    );
    Ok(bytes)
}

fn missing(error: &anyhow::Error) -> bool {
    error
        .downcast_ref::<std::io::Error>()
        .is_some_and(|error| error.kind() == std::io::ErrorKind::NotFound)
}

fn working_files(root: &Path, private: bool) -> Result<Files> {
    #[cfg(windows)]
    let stages = git(root, &["ls-files", "--stage", "-z"], private)?;
    #[cfg(windows)]
    let executable: BTreeSet<_> = stages
        .split(|byte| *byte == 0)
        .filter_map(|entry| {
            let (header, path) = std::str::from_utf8(entry).ok()?.split_once('\t')?;
            header.starts_with("100755 ").then_some(path)
        })
        .collect();
    let names = crate::local::paths(&git(
        root,
        &[
            "ls-files",
            "--cached",
            "--others",
            "--exclude-standard",
            "-z",
        ],
        private,
    )?)?;
    let mut files = Files::new();
    let mut size = 0_u64;
    for name in names {
        let path = root.join(&name);
        if path.try_exists()? {
            ensure!(
                dunce::canonicalize(&path)?.starts_with(dunce::canonicalize(root)?),
                "project input escapes its source"
            );
        }
        match bounded(&path, 64 * 1024 * 1024) {
            Ok(bytes) => {
                size = size
                    .checked_add(bytes.len() as u64)
                    .context("source budget overflow")?;
                ensure!(size <= 64 * 1024 * 1024, "project source exceeds 64 MiB");
                #[cfg(unix)]
                let is_executable = {
                    use std::os::unix::fs::PermissionsExt;
                    fs::metadata(&path)?.permissions().mode() & 0o111 != 0
                };
                #[cfg(windows)]
                let is_executable = executable.contains(name.as_str());
                files.insert(
                    name,
                    Input {
                        bytes,
                        executable: is_executable,
                    },
                );
            }
            Err(error) if missing(&error) => {}
            Err(error) => {
                return Err(error).with_context(|| format!("{name}: unbound project source"));
            }
        }
    }
    Ok(files)
}

fn matches(path: &str, input: &str) -> bool {
    input == "." || path == input || input.ends_with('/') && path.starts_with(input)
}

fn support(path: &str) -> bool {
    path == CONTRACT
        || path.starts_with(".github/")
        || path.starts_with(".cargo/")
        || matches!(
            path,
            "Cargo.toml"
                | "Cargo.lock"
                | "go.mod"
                | "go.sum"
                | "Package.swift"
                | "Package.resolved"
                | "bun.lock"
                | "bun.lockb"
                | "lean-toolchain"
                | ".gitignore"
                | ".gitattributes"
                | "mise.toml"
                | ".mise.toml"
                | "mise.lock"
                | "Justfile"
                | "justfile"
                | "rust-toolchain"
                | "rust-toolchain.toml"
        )
        || matches!(
            Path::new(path).extension().and_then(|value| value.to_str()),
            Some(
                "toml"
                    | "json"
                    | "yaml"
                    | "yml"
                    | "csproj"
                    | "fsproj"
                    | "props"
                    | "targets"
                    | "cabal"
            )
        )
}

fn declared_source(path: &str) -> bool {
    matches!(
        Path::new(path).extension().and_then(|value| value.to_str()),
        Some(
            "rs" | "go"
                | "cs"
                | "swift"
                | "ts"
                | "tsx"
                | "js"
                | "jsx"
                | "ml"
                | "mli"
                | "hs"
                | "lean"
                | "py"
                | "sh"
                | "ps1"
        )
    )
}

fn validate(contract: &Contract, files: &Files) -> Result<()> {
    ensure!(
        contract.version == 1 && !contract.checks.is_empty() && !contract.ci.is_empty(),
        "project verification contract is empty or unsupported"
    );
    let mut identities = BTreeSet::new();
    for check in &contract.checks {
        ensure!(
            literal(&check.id) && identities.insert(&check.id),
            "project check identities are invalid or duplicated"
        );
        ensure!(
            !check.command.is_empty()
                && check
                    .command
                    .iter()
                    .all(|part| !part.is_empty() && !part.contains(['\0', '\n', '\r'])),
            "project command must be an explicit argument vector"
        );
        ensure!(
            super::command::permitted(&check.command),
            "project command must be a supported verification operation"
        );
        ensure!(
            budget_valid(check.phase, check.timeout_seconds),
            "project check exceeds its automatic phase budget"
        );
        let platforms: BTreeSet<_> = check.platforms.iter().collect();
        ensure!(
            !platforms.is_empty()
                && platforms.len() == check.platforms.len()
                && platforms
                    .iter()
                    .all(|value| matches!(value.as_str(), "linux" | "macos" | "windows")),
            "project platforms are invalid"
        );
        ensure!(!check.inputs.is_empty(), "project input coverage is empty");
        for input in &check.inputs {
            ensure!(
                input == "."
                    || !input.is_empty()
                        && crate::local::paths(
                            format!("{}\0", input.trim_end_matches('/')).as_bytes()
                        )
                        .is_ok(),
                "project input scope is invalid"
            );
            ensure!(
                files.keys().any(|path| matches(path, input)),
                "project input scope has no source: {input}"
            );
        }
        ensure!(
            check
                .tools
                .iter()
                .all(|tool| tool
                    .rsplit_once('@')
                    .is_some_and(|(name, version)| !name.is_empty()
                        && !version.is_empty()
                        && !tool.contains(['\0', '\n', '\r', ' ']))),
            "project tool selectors must be explicit"
        );
        ensure!(
            check.environment.iter().all(|name| !name.is_empty()
                && name.bytes().all(|byte| byte.is_ascii_uppercase()
                    || byte.is_ascii_digit()
                    || byte == b'_')),
            "project environment identities are invalid"
        );
    }
    for path in files
        .keys()
        .filter(|path| declared_source(path) && !support(path))
    {
        ensure!(
            contract
                .checks
                .iter()
                .any(|check| check.inputs.iter().any(|input| matches(path, input))),
            "project source has no verification scope: {path}"
        );
    }
    let mut covered = BTreeSet::new();
    let mut bindings = BTreeSet::new();
    for ci in &contract.ci {
        ensure!(
            literal(&ci.workflow)
                && matches!(
                    Path::new(&ci.workflow)
                        .extension()
                        .and_then(|value| value.to_str()),
                    Some("yml" | "yaml")
                )
                && literal(&ci.job),
            "CI binding identity is invalid"
        );
        ensure!(
            bindings.insert((&ci.workflow, &ci.job)) && !ci.checks.is_empty(),
            "CI check coverage is empty or duplicated"
        );
        let source = files
            .get(&format!(".github/workflows/{}", ci.workflow))
            .context("bound CI workflow is missing")?;
        let documents = YamlLoader::load_from_str(std::str::from_utf8(&source.bytes)?)?;
        ensure!(documents.len() == 1, "bound CI workflow is invalid");
        let job = &documents[0]["jobs"][ci.job.as_str()];
        ensure!(job.as_hash().is_some(), "bound CI job is missing");
        let entry = format!("mise x -- just check {}::{}", ci.workflow, ci.job);
        ensure!(
            unsuppressed(job),
            "authoritative CI job cannot suppress its outcome"
        );
        ensure!(
            job["steps"]
                .as_vec()
                .is_some_and(|steps| steps.iter().any(|step| unsuppressed(step)
                    && step["run"].as_str().is_some_and(|run| run.trim() == entry))),
            "CI job does not invoke the authoritative project gate: {}::{}",
            ci.workflow,
            ci.job
        );
        for id in &ci.checks {
            let check = contract
                .checks
                .iter()
                .find(|check| &check.id == id)
                .context("CI binding refers to an unknown check")?;
            for platform in &check.platforms {
                if runners(job)?.contains(platform) {
                    covered.insert((&check.id, platform));
                }
            }
        }
    }
    for hosted in &contract.hosted {
        ensure!(
            literal(&hosted.workflow)
                && literal(&hosted.job)
                && bindings.insert((&hosted.workflow, &hosted.job)),
            "hosted CI binding is invalid or duplicated"
        );
        let source = files
            .get(&format!(".github/workflows/{}", hosted.workflow))
            .context("hosted workflow is missing")?;
        let documents = YamlLoader::load_from_str(std::str::from_utf8(&source.bytes)?)?;
        ensure!(documents.len() == 1, "hosted workflow is invalid");
        let job = &documents[0]["jobs"][hosted.job.as_str()];
        ensure!(
            job.as_hash().is_some()
                && capability_valid(&hosted.capability, &hosted.workflow, &documents[0], job),
            "job does not establish its declared hosted capability"
        );
    }
    for (path, source) in files.iter().filter(|(path, _)| {
        path.starts_with(".github/workflows/")
            && matches!(
                Path::new(path).extension().and_then(|value| value.to_str()),
                Some("yml" | "yaml")
            )
    }) {
        let documents = YamlLoader::load_from_str(std::str::from_utf8(&source.bytes)?)?;
        ensure!(documents.len() == 1, "workflow coverage is invalid");
        let jobs = documents[0]["jobs"]
            .as_hash()
            .context("workflow has no job inventory")?;
        ensure!(!jobs.is_empty(), "workflow coverage is empty");
        let workflow = Path::new(path)
            .file_name()
            .and_then(|value| value.to_str())
            .context("workflow filename is invalid")?;
        for (job, _) in jobs {
            let job = job.as_str().context("workflow job identity is invalid")?;
            ensure!(
                bindings
                    .iter()
                    .any(|(file, id)| file.as_str() == workflow && id.as_str() == job),
                "CI job has no verification contract: {workflow}::{job}"
            );
        }
    }
    let coverage = contract
        .checks
        .iter()
        .flat_map(|check| {
            check
                .platforms
                .iter()
                .map(move |platform| (&check.id, platform))
        })
        .fold(Coverage::Empty, |coverage, required| {
            coverage.observe(covered.contains(&required))
        });
    ensure!(
        coverage.complete(),
        "CI does not cover every declared project check and platform"
    );
    Ok(())
}

fn unsuppressed(value: &Yaml) -> bool {
    value["if"].is_badvalue()
        && (value["continue-on-error"].is_badvalue()
            || value["continue-on-error"] == Yaml::Boolean(false))
}

fn entrypoint(root: &Path) -> Result<()> {
    let output = private_command("mise", root)
        .env("MISE_AUTO_INSTALL", "false")
        .env("MISE_TRUSTED_CONFIG_PATHS", root)
        .args([
            "x",
            "just@1.58.0",
            "--",
            "just",
            "--dump",
            "--dump-format",
            "json",
        ])
        .output()?;
    ensure!(
        output.status.success(),
        "project needs a valid just command surface: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let parsed = crate::json::parse(&output.stdout)?;
    let recipe = &parsed["recipes"]["check"];
    let expected = serde_json::json!([[
        "ci-policy project-check --suite ",
        [["call", "quote", ["variable", "suite"]]]
    ]]);
    ensure!(
        recipe["body"] == expected
            && recipe["dependencies"] == serde_json::json!([])
            && recipe["attributes"] == serde_json::json!([])
            && recipe["shebang"] == false
            && recipe["private"] == false
            && recipe["parameters"]
                .as_array()
                .is_some_and(|parameters| parameters.len() == 1
                    && parameters[0]["name"] == "suite"
                    && parameters[0]["default"] == "local"
                    && parameters[0]["kind"] == "singular"
                    && parameters[0]["export"] == false
                    && parameters[0]["flag"] == false
                    && parameters[0]["multiple"] == false)
            && parsed["settings"]["dotenv_load"] == false
            && parsed["settings"]["export"] == false
            && parsed["settings"]["working_directory"].is_null()
            && parsed["settings"]["positional_arguments"] == false
            && parsed["settings"]["no_cd"] == false
            && parsed["settings"]["fallback"] == false
            && parsed["settings"]["windows_powershell"] == false
            && shell(&parsed["settings"]["shell"])
            && shell(&parsed["settings"]["windows_shell"])
            && parsed["assignments"]
                .as_object()
                .is_some_and(|assignments| assignments
                    .values()
                    .all(|assignment| assignment["export"] == false)),
        "just check must invoke the shared project gate directly"
    );
    Ok(())
}

fn shell(value: &serde_json::Value) -> bool {
    value.is_null()
        || value == &serde_json::json!({"command":"bash","arguments":["-eu","-o","pipefail","-c"]})
        || value == &serde_json::json!({"command":"sh","arguments":["-eu","-c"]})
}

fn capability_valid(capability: &Capability, workflow: &str, document: &Yaml, job: &Yaml) -> bool {
    let steps = job["steps"].as_vec();
    let uses = |prefix: &str| {
        steps.is_some_and(|steps| {
            steps.iter().any(|step| {
                step["uses"].as_str().is_some_and(|value| {
                    value.starts_with(prefix) && crate::immutable_reference(value)
                })
            })
        })
    };
    let runs = |needle: &str| {
        steps.is_some_and(|steps| {
            steps.iter().any(|step| {
                step["run"]
                    .as_str()
                    .is_some_and(|value| value.contains(needle))
            })
        })
    };
    match capability {
        Capability::Security => {
            uses("github/codeql-action/")
                || uses("ossf/scorecard-action@")
                || uses("actions/dependency-review-action@")
        }
        Capability::Policy => {
            job["uses"].as_str().is_some_and(|value| {
                crate::immutable_reference(value)
                    && (value.starts_with("P4suta/project-template/.github/workflows/")
                        || value.starts_with("$/.github/workflows/"))
                    && (value.contains("common-policy.yml")
                        || value.contains("ci-policy-checks.yml"))
            }) || workflow == "common-policy.yml"
                && uses("$/.github/actions/ci-policy")
                && runs(" check ")
        }
        Capability::Aggregate => aggregate_valid(job),
        Capability::Approval => !job["environment"].is_badvalue(),
        Capability::Campaign => document["on"].as_hash().is_some_and(|events| {
            !events.is_empty()
                && events
                    .keys()
                    .all(|event| matches!(event.as_str(), Some("schedule" | "workflow_dispatch")))
        }),
        Capability::Initialization => {
            workflow == "init.yml"
                && job["if"].as_str().is_some_and(|condition| {
                    condition.trim() == "github.event.repository.name != 'project-template'"
                })
        }
    }
}

fn aggregate_valid(job: &Yaml) -> bool {
    let needs = if let Some(value) = job["needs"].as_str() {
        vec![value]
    } else if let Some(values) = job["needs"].as_vec() {
        let Some(needs) = values.iter().map(Yaml::as_str).collect::<Option<Vec<_>>>() else {
            return false;
        };
        needs
    } else {
        return false;
    };
    if needs.is_empty()
        || job["if"].as_str() != Some("always()")
        || !(job["continue-on-error"].is_badvalue()
            || job["continue-on-error"] == Yaml::Boolean(false))
    {
        return false;
    }
    job["steps"].as_vec().is_some_and(|steps| {
        let builds_gate = steps.iter().any(|step| {
            step["id"].as_str() == Some("policy")
                && unsuppressed(step)
                && step["uses"].as_str() == Some("$/.github/actions/ci-policy")
        });
        builds_gate
            && steps.iter().any(|step| {
                if !unsuppressed(step)
                    || step["env"]["POLICY"].as_str()
                        != Some("${{ steps.policy.outputs.executable }}")
                    || step["env"]["RESULTS"].as_str() != Some("${{ toJSON(needs) }}")
                {
                    return false;
                }
                let Some(required) = step["run"]
                    .as_str()
                    .and_then(|run| {
                        run.trim()
                            .strip_prefix("\"$POLICY\" gate --needs \"$RESULTS\" --require-json '")
                    })
                    .and_then(|value| value.strip_suffix('\''))
                else {
                    return false;
                };
                crate::json::parse(required.as_bytes())
                    .ok()
                    .and_then(|value| serde_json::from_value::<Vec<String>>(value).ok())
                    .is_some_and(|required| {
                        required.len() == needs.len()
                            && required.iter().map(String::as_str).collect::<BTreeSet<_>>()
                                == needs.iter().copied().collect::<BTreeSet<_>>()
                    })
            })
    })
}

fn runners(job: &Yaml) -> Result<BTreeSet<String>> {
    ensure!(
        job["strategy"]["matrix"]["exclude"].is_badvalue()
            && job["strategy"]["matrix"]["include"].is_badvalue(),
        "native CI platform coverage cannot be overridden by matrix exceptions"
    );
    let values = match job["runs-on"].as_str() {
        Some("${{ matrix.os }}") => job["strategy"]["matrix"]["os"]
            .as_vec()
            .context("CI runner matrix is unavailable")?
            .iter()
            .map(|value| value.as_str().context("CI runner is not literal"))
            .collect::<Result<Vec<_>>>()?,
        Some(value) => vec![value],
        None => anyhow::bail!("CI runner must be a supported native runner or OS matrix"),
    };
    values
        .into_iter()
        .map(|value| {
            Ok(if value.starts_with("ubuntu-") {
                "linux"
            } else if value.starts_with("macos-") {
                "macos"
            } else if value.starts_with("windows-") {
                "windows"
            } else {
                anyhow::bail!("CI runner has no declared native platform")
            }
            .to_owned())
        })
        .collect()
}

fn hash_parts(parts: impl IntoIterator<Item = impl AsRef<[u8]>>) -> String {
    let mut hash = Sha256::new();
    for part in parts {
        let bytes = part.as_ref();
        hash.update((bytes.len() as u64).to_le_bytes());
        hash.update(bytes);
    }
    format!("{:x}", hash.finalize())
}

fn environment(check: &Check) -> BTreeMap<OsString, OsString> {
    std::env::vars_os()
        .filter(|(key, _)| {
            let name = key.to_string_lossy();
            let upper = name.to_ascii_uppercase();
            !matches!(
                upper.as_str(),
                "CARGO_TARGET_DIR"
                    | "CARGO_BUILD_JOBS"
                    | "MISE_TRUSTED_CONFIG_PATHS"
                    | "MISE_AUTO_INSTALL"
            ) && (matches!(
                upper.as_str(),
                "HOME"
                    | "USERPROFILE"
                    | "PATH"
                    | "PATHEXT"
                    | "TEMP"
                    | "TMP"
                    | "TMPDIR"
                    | "SYSTEMROOT"
                    | "WINDIR"
                    | "SYSTEMDRIVE"
                    | "COMSPEC"
                    | "APPDATA"
                    | "LOCALAPPDATA"
                    | "PROGRAMDATA"
                    | "PROGRAMFILES"
                    | "PROGRAMFILES(X86)"
                    | "PROGRAMW6432"
                    | "COMMONPROGRAMFILES"
                    | "COMMONPROGRAMFILES(X86)"
                    | "COMMONPROGRAMW6432"
                    | "LANG"
                    | "LC_ALL"
                    | "TERM"
                    | "CI"
            ) || upper.starts_with("CARGO_")
                || upper.starts_with("RUST")
                || upper.starts_with("GO")
                || upper.starts_with("DOTNET_")
                || upper.starts_with("MISE_")
                || check
                    .environment
                    .iter()
                    .any(|declared| declared == name.as_ref()))
        })
        .collect()
}

fn tool_identity(root: &Path, check: &Check) -> Result<(Vec<u8>, std::path::PathBuf)> {
    let output = private_command("mise", root)
        .env_clear()
        .envs(environment(check))
        .env("MISE_AUTO_INSTALL", "false")
        .env("MISE_TRUSTED_CONFIG_PATHS", root)
        .arg("ls")
        .args(["--current", "--json"])
        .output()?;
    ensure!(
        output.status.success(),
        "project tool identities could not be established"
    );
    let value = crate::json::parse(&output.stdout)?;
    let tools = value.as_object().context("project tools must be a map")?.iter().map(|(name, entries)| {
        let versions = entries.as_array().context("project tool versions must be an array")?.iter().map(|tool| serde_json::json!({"version":tool["version"], "install_path":tool["install_path"]})).collect::<Vec<_>>();
        Ok((name.clone(), versions))
    }).collect::<Result<BTreeMap<_, _>>>()?;
    let mut identity = serde_json::to_vec(&tools)?;
    identity.extend_from_slice(&serde_json::to_vec(&check.tools)?);
    for selector in &check.tools {
        let (name, version) = selector
            .rsplit_once('@')
            .context("project selector is invalid")?;
        let output = private_command("mise", root)
            .env_clear()
            .envs(environment(check))
            .env("MISE_AUTO_INSTALL", "false")
            .env("MISE_TRUSTED_CONFIG_PATHS", root)
            .args(["ls", "--json", name])
            .output()?;
        ensure!(
            output.status.success(),
            "required tool inventory is unavailable"
        );
        let value = crate::json::parse(&output.stdout)?;
        let installed = value
            .as_array()
            .context("required tool inventory must be an array")?
            .iter()
            .find(|entry| entry["version"].as_str() == Some(version) && entry["installed"] == true)
            .context("required pinned tool is not installed; prepare tools before running hooks")?;
        let path = Path::new(
            installed["install_path"]
                .as_str()
                .context("required tool has no installation path")?,
        );
        identity.extend_from_slice(&serde_json::to_vec(
            installed
                .get("version")
                .context("required tool version is missing")?,
        )?);
        if let Some(crate_name) = name.strip_prefix("cargo:") {
            let executable = if crate_name == "kani-verifier" {
                "cargo-kani"
            } else {
                crate_name
            };
            let path = path.join("bin").join(if cfg!(windows) {
                format!("{executable}.exe")
            } else {
                executable.to_owned()
            });
            let mut input = fs::File::open(path).context("required Cargo extension is missing")?;
            let mut hash = Sha256::new();
            let mut bytes = [0_u8; 65536];
            loop {
                let count = input.read(&mut bytes)?;
                if count == 0 {
                    break;
                }
                hash.update(&bytes[..count]);
            }
            identity.extend_from_slice(&hash.finalize());
        }
    }
    let output = private_command("mise", root)
        .env_clear()
        .envs(environment(check))
        .env("MISE_AUTO_INSTALL", "false")
        .env("MISE_TRUSTED_CONFIG_PATHS", root)
        .args(["config", "ls", "--json"])
        .output()?;
    ensure!(
        output.status.success(),
        "mise configuration scope is unavailable"
    );
    let value = crate::json::parse(&output.stdout)?;
    let configs = value
        .as_array()
        .context("mise configuration inventory must be an array")?;
    let mut external = BTreeMap::new();
    for config in configs {
        let path = dunce::canonicalize(Path::new(
            config["path"]
                .as_str()
                .context("mise configuration has no path")?,
        ))?;
        if !path.starts_with(root) {
            external.insert(
                path,
                bounded(
                    Path::new(
                        config["path"]
                            .as_str()
                            .context("mise configuration path is invalid")?,
                    ),
                    1024 * 1024,
                )?,
            );
        }
    }
    for (path, bytes) in external {
        identity.extend_from_slice(
            hash_parts([path.as_os_str().as_encoded_bytes(), &bytes]).as_bytes(),
        );
    }
    let executable = std::env::current_exe()?;
    let mut file = fs::File::open(executable)?;
    let mut hash = Sha256::new();
    let mut buffer = [0_u8; 65536];
    loop {
        let count = file.read(&mut buffer)?;
        if count == 0 {
            break;
        }
        hash.update(&buffer[..count]);
    }
    identity.extend_from_slice(&hash.finalize());
    let program = if check.command[0] == "ci-policy" {
        std::env::current_exe()?
    } else {
        let mut resolved = None;
        for selector in check.tools.iter().map(Some).chain(std::iter::once(None)) {
            let mut command = private_command("mise", root);
            command
                .env_clear()
                .envs(environment(check))
                .env("MISE_TRUSTED_CONFIG_PATHS", root)
                .env("MISE_AUTO_INSTALL", "false")
                .args(["which", &check.command[0]]);
            if let Some(selector) = selector {
                command.args(["--tool", selector]);
            }
            let output = command.output()?;
            if output.status.success() {
                resolved = Some(std::path::PathBuf::from(
                    std::str::from_utf8(&output.stdout)?.trim(),
                ));
                break;
            }
        }
        resolved
            .or_else(|| {
                let search = std::env::var_os("PATH")?;
                let executable = if cfg!(windows) {
                    format!("{}.exe", check.command[0])
                } else {
                    check.command[0].clone()
                };
                std::env::split_paths(&search)
                    .filter(|directory| directory.file_name().is_none_or(|name| name != "shims"))
                    .map(|directory| directory.join(&executable))
                    .find(|path| path.is_file())
            })
            .context("required project executable is unavailable")?
    };
    ensure!(
        program.is_absolute() && program.is_file(),
        "required project executable is not a file"
    );
    identity.extend_from_slice(
        program
            .file_name()
            .context("project executable has no dispatch name")?
            .as_encoded_bytes(),
    );
    let mut file = fs::File::open(&program)?;
    let mut hash = Sha256::new();
    loop {
        let count = file.read(&mut buffer)?;
        if count == 0 {
            break;
        }
        hash.update(&buffer[..count]);
    }
    identity.extend_from_slice(&hash.finalize());
    Ok((identity, program))
}

fn identity(check: &Check, files: &Files, tools: &[u8]) -> Result<String> {
    let mut parts = vec![
        serde_json::to_vec(check)?,
        tools.to_vec(),
        std::env::consts::OS.as_bytes().to_vec(),
        std::env::consts::ARCH.as_bytes().to_vec(),
    ];
    for (path, input) in files
        .iter()
        .filter(|(path, _)| support(path) || check.inputs.iter().any(|input| matches(path, input)))
    {
        parts.push(path.as_bytes().to_vec());
        parts.push(vec![u8::from(input.executable)]);
        parts.push(input.bytes.clone());
    }
    for name in &check.environment {
        parts.push(name.as_bytes().to_vec());
        let value = std::env::var_os(name);
        parts.push(vec![u8::from(value.is_some())]);
        parts.push(value.map_or_else(Vec::new, |value| value.as_encoded_bytes().to_vec()));
    }
    for (key, value) in environment(check) {
        parts.push(key.as_encoded_bytes().to_vec());
        parts.push(value.as_encoded_bytes().to_vec());
    }
    Ok(hash_parts(parts))
}

fn execute(root: &Path, check: &Check, program: &Path, target: &Path) -> Result<()> {
    let mut command = private_command("mise", root);
    command
        .env_clear()
        .envs(environment(check))
        .arg("x")
        .args(&check.tools)
        .arg("--")
        .arg(program)
        .args(&check.command[1..])
        .env("MISE_TRUSTED_CONFIG_PATHS", root)
        .env("MISE_AUTO_INSTALL", "false")
        .env("CARGO_BUILD_JOBS", "2")
        .env("CARGO_TARGET_DIR", target)
        .stdin(Stdio::null());
    let mut wrapped = CommandWrap::from(command);
    #[cfg(unix)]
    wrapped.wrap(process_wrap::std::ProcessGroup::leader());
    #[cfg(windows)]
    wrapped.wrap(process_wrap::std::JobObject);
    let mut child = wrapped.spawn().context("project check could not start")?;
    let start = Instant::now();
    loop {
        let result = child.try_wait();
        let completion = match &result {
            Ok(Some(status)) if status.success() => Completion::Success,
            Ok(Some(_)) => Completion::Failure,
            Ok(None) => Completion::Running,
            Err(_) => Completion::Error,
        };
        match progress(
            completion,
            start.elapsed() >= Duration::from_secs(check.timeout_seconds),
        ) {
            Progress::Accept => {
                cleanup(child.as_mut())?;
                return Ok(());
            }
            Progress::Reject => {
                if let Err(error) = cleanup(child.as_mut()) {
                    eprintln!("project process group cleanup failed: {error:#}");
                }
                return Err(anyhow::anyhow!(
                    "project check {} failed ({:?})",
                    check.id,
                    result?
                ));
            }
            Progress::Wait => thread::sleep(Duration::from_millis(20)),
            Progress::Timeout | Progress::Error => {
                let primary = match result {
                    Err(error) => {
                        anyhow::Error::new(error).context("project check could not be awaited")
                    }
                    Ok(_) => {
                        anyhow::anyhow!("project check {} exceeded its declared budget", check.id)
                    }
                };
                if let Err(error) = cleanup(child.as_mut()) {
                    eprintln!("project process group cleanup failed: {error:#}");
                }
                return Err(primary);
            }
        }
    }
}

fn cleanup(child: &mut dyn ChildWrapper) -> Result<()> {
    if let Err(error) = child.start_kill() {
        #[cfg(unix)]
        let absent = error.raw_os_error() == Some(nix::errno::Errno::ESRCH as i32);
        #[cfg(not(unix))]
        let absent = false;
        if !absent {
            return Err(error).context("project process group could not be terminated");
        }
    }
    let start = Instant::now();
    loop {
        match child
            .try_wait()
            .context("project process group could not be reaped")?
        {
            Some(_) => return Ok(()),
            None if start.elapsed() < Duration::from_secs(2) => {
                thread::sleep(Duration::from_millis(20));
            }
            None => anyhow::bail!("project process group exceeded its cleanup budget"),
        }
    }
}

fn evaluate(
    root: &Path,
    files: &Files,
    phase: Phase,
    suite: &str,
    cache: &Path,
    target: &Path,
) -> Result<()> {
    let contract: Contract = serde_json::from_value(crate::json::parse(
        &files
            .get(CONTRACT)
            .context("project needs .ci/verification.json with CI-equivalent checks")?
            .bytes,
    )?)?;
    validate(&contract, files)?;
    entrypoint(root)?;
    let selected = if suite == "local" {
        None
    } else {
        Some(
            &contract
                .ci
                .iter()
                .find(|ci| format!("{}::{}", ci.workflow, ci.job) == suite)
                .context("unknown CI check suite")?
                .checks,
        )
    };
    ensure!(
        selected.is_none() || phase == Phase::Development,
        "CI suites require the complete verification phase"
    );
    let mut budget = Budget::new(phase);
    for check in &contract.checks {
        if selected.is_none_or(|ids| ids.contains(&check.id))
            && (phase == Phase::Development || check.phase == Phase::Commit)
            && check
                .platforms
                .iter()
                .any(|platform| platform == std::env::consts::OS)
        {
            ensure!(
                budget.reserve(check.timeout_seconds),
                "complete project plan exceeds its automatic phase budget"
            );
        }
    }
    let mut applicable = 0;
    for check in &contract.checks {
        if selected.is_some_and(|ids| !ids.contains(&check.id)) {
            continue;
        }
        let preliminary = decide(
            phase,
            check.phase,
            check
                .platforms
                .iter()
                .any(|platform| platform == std::env::consts::OS),
            Cache::Always,
            false,
        );
        if preliminary == Decision::NotSelected {
            continue;
        }
        if preliminary == Decision::OtherPlatform {
            println!(
                "{}",
                serde_json::json!({"check":check.id,"outcome":"other_platform","platform":std::env::consts::OS})
            );
            continue;
        }
        let (tools, program) = tool_identity(root, check)?;
        let identity = identity(check, files, &tools)?;
        let receipt = cache.join(format!("{identity}.json"));
        let success = match bounded(&receipt, 1024) {
            Ok(bytes) => serde_json::from_value::<Success>(crate::json::parse(&bytes)?)
                .is_ok_and(|success| success.identity == identity && success.check == check.id),
            Err(error) if missing(&error) => false,
            Err(error) => return Err(error).context("project evidence is not a regular receipt"),
        };
        let decision = decide(
            phase,
            check.phase,
            check
                .platforms
                .iter()
                .any(|platform| platform == std::env::consts::OS),
            check.cache,
            success,
        );
        let outcome = match decision {
            Decision::NotSelected => continue,
            Decision::OtherPlatform => "other_platform",
            Decision::Reuse => {
                applicable += 1;
                "reused"
            }
            Decision::Run => {
                applicable += 1;
                execute(root, check, &program, target)?;
                ensure!(
                    working_files(root, true)? == *files,
                    "project source changed during verification"
                );
                ensure!(
                    tool_identity(root, check)?.0 == tools,
                    "project tool identity changed during verification"
                );
                fs::create_dir_all(cache)?;
                let temporary = tempfile::NamedTempFile::new_in(cache)?;
                serde_json::to_writer(
                    temporary.as_file(),
                    &Success {
                        check: check.id.clone(),
                        identity: identity.clone(),
                    },
                )?;
                temporary.as_file().sync_all()?;
                temporary
                    .persist(&receipt)
                    .context("project success evidence could not be published")?;
                "ran"
            }
        };
        println!(
            "{}",
            serde_json::json!({"check":check.id,"outcome":outcome,"platform":std::env::consts::OS,"inputs":identity})
        );
    }
    ensure!(
        phase == Phase::Commit || applicable != 0,
        "project has no applicable completed local checks"
    );
    Ok(())
}

pub fn run(
    root: &Path,
    scope: Scope<'_>,
    phase: Phase,
    suite: &str,
    cache: Option<&Path>,
) -> Result<()> {
    snapshot(root, scope, phase, suite, cache, |_| Ok(()))
}

pub fn verify_index(root: &Path) -> Result<()> {
    snapshot(root, Scope::Index, Phase::Commit, "local", None, |_| Ok(()))
}

pub fn verify_revision(
    root: &Path,
    revision: &str,
    after: impl FnOnce(&Path) -> Result<()>,
) -> Result<()> {
    let revision = Revision::parse(revision)?;
    snapshot(
        root,
        Scope::Revision(revision),
        Phase::Development,
        "local",
        None,
        after,
    )
}

fn snapshot(
    root: &Path,
    scope: Scope<'_>,
    phase: Phase,
    suite: &str,
    cache: Option<&Path>,
    after: impl FnOnce(&Path) -> Result<()>,
) -> Result<()> {
    let root = dunce::canonicalize(root)?;
    ensure!(
        dunce::canonicalize(Path::new(
            std::str::from_utf8(&git(&root, &["rev-parse", "--show-toplevel"], false)?)?.trim()
        ))? == root,
        "project root must be the Git repository root"
    );
    let owner = match scope {
        Scope::Working => Some(working_files(&root, false)?),
        Scope::Index | Scope::Revision(_) => None,
    };
    let initial_index = if matches!(scope, Scope::Index) {
        Some(git(&root, &["write-tree"], false)?)
    } else {
        None
    };
    let tree = match scope {
        Scope::Working => None,
        Scope::Index => Some(
            std::str::from_utf8(initial_index.as_ref().context("project index is missing")?)?
                .trim()
                .to_owned(),
        ),
        Scope::Revision(revision) => {
            ensure!(
                git(&root, &["cat-file", "-t", revision.0], false)? == b"commit\n",
                "project revision must identify a commit"
            );
            Some(revision.0.to_owned())
        }
    };
    let temporary = tempfile::Builder::new().prefix("ci-project-").tempdir()?;
    let candidate = temporary.path().join("candidate");
    let output = private_command("git", &root)
        .args([
            "clone",
            "--shared",
            "--no-checkout",
            "--quiet",
            "--template=",
            "--",
        ])
        .arg(&root)
        .arg(&candidate)
        .output()?;
    ensure!(
        output.status.success(),
        "private project checkout could not be created: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let candidate = dunce::canonicalize(candidate)?;
    if let Some(tree) = &tree {
        let entries = git(&candidate, &["ls-tree", "-r", "-z", tree], true)?;
        ensure!(
            entries
                .split(|byte| *byte == 0)
                .filter(|entry| !entry.is_empty())
                .all(|entry| entry.starts_with(b"100644 blob ")
                    || entry.starts_with(b"100755 blob ")),
            "project source contains an unbound link or external repository"
        );
        git(&candidate, &["read-tree", "--reset", "-u", tree], true)?;
    } else {
        if git(&candidate, &["rev-parse", "--verify", "HEAD"], true).is_ok() {
            git(&candidate, &["read-tree", "--reset", "-u", "HEAD"], true)?;
        }
        let files = owner
            .as_ref()
            .context("working project source is missing")?;
        for path in crate::local::paths(&git(&candidate, &["ls-files", "-z"], true)?)? {
            if !files.contains_key(&path) {
                fs::remove_file(candidate.join(path))?;
            }
        }
        for (path, input) in files {
            let path = candidate.join(path);
            fs::create_dir_all(path.parent().context("source has no parent")?)?;
            match fs::symlink_metadata(&path) {
                Ok(_) => fs::remove_file(&path)?,
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
                Err(error) => return Err(error.into()),
            }
            fs::write(&path, &input.bytes)?;
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                fs::set_permissions(
                    path,
                    fs::Permissions::from_mode(if input.executable { 0o755 } else { 0o644 }),
                )?;
            }
        }
        git(&candidate, &["add", "--all", "--", "."], true)?;
        for executable in [false, true] {
            let mut paths = Vec::new();
            for (path, _) in files
                .iter()
                .filter(|(_, input)| input.executable == executable)
            {
                ensure!(
                    super::index::append_path(path.as_bytes(), &mut paths),
                    "Git index path contains an invalid delimiter"
                );
            }
            if !paths.is_empty() {
                crate::local::execute(
                    private_command("git", &candidate).args([
                        "-c",
                        "core.fsmonitor=false",
                        "update-index",
                        if executable {
                            "--chmod=+x"
                        } else {
                            "--chmod=-x"
                        },
                        "-z",
                        "--stdin",
                    ]),
                    Some(&paths),
                )
                .context("cannot establish project source index modes")?;
            }
        }
    }
    if let Scope::Revision(revision) = scope {
        git(
            &candidate,
            &["update-ref", "--no-deref", "HEAD", revision.0],
            true,
        )?;
    } else {
        let tree = git(&candidate, &["write-tree"], true)?;
        let mut command = private_command("git", &candidate);
        command
            .args([
                "-c",
                "commit.gpgsign=false",
                "commit-tree",
                std::str::from_utf8(&tree)?.trim(),
                "-m",
                "Verification snapshot",
            ])
            .env("GIT_AUTHOR_NAME", "ci-policy")
            .env("GIT_AUTHOR_EMAIL", "verification@example.invalid")
            .env("GIT_COMMITTER_NAME", "ci-policy")
            .env("GIT_COMMITTER_EMAIL", "verification@example.invalid")
            .env("GIT_AUTHOR_DATE", "2000-01-01T00:00:00Z")
            .env("GIT_COMMITTER_DATE", "2000-01-01T00:00:00Z");
        if let Ok(parent) = git(&candidate, &["rev-parse", "--verify", "HEAD"], true) {
            command.args(["-p", std::str::from_utf8(&parent)?.trim()]);
        }
        let output = command.output()?;
        ensure!(
            output.status.success(),
            "verification snapshot could not be bound to HEAD"
        );
        git(
            &candidate,
            &[
                "update-ref",
                "--no-deref",
                "HEAD",
                std::str::from_utf8(&output.stdout)?.trim(),
            ],
            true,
        )?;
    }
    let files = working_files(&candidate, true)?;
    let default_cache = crate::hooks::home()?.join(".cache/ci-policy/project");
    let cache = cache.map_or(default_cache, Path::to_path_buf);
    let cache = if cache.is_absolute() {
        cache
    } else {
        root.join(cache)
    };
    let target = std::env::var_os("CARGO_TARGET_DIR")
        .map(std::path::PathBuf::from)
        .map(|path| {
            if path.is_absolute() {
                path
            } else {
                root.join(path)
            }
        })
        .unwrap_or_else(|| cache.join("targets"));
    evaluate(&candidate, &files, phase, suite, &cache, &target)?;
    after(&candidate)?;
    ensure!(
        working_files(&candidate, true)? == files,
        "project inputs changed after verification"
    );
    if let Some(owner) = owner {
        ensure!(
            working_files(&root, false)? == owner,
            "original working inputs changed during verification"
        );
    }
    if let Some(index) = initial_index {
        ensure!(
            git(&root, &["write-tree"], false)? == index,
            "original staged inputs changed during verification"
        );
    }
    temporary
        .close()
        .context("private project checkout could not be removed")?;
    Ok(())
}
