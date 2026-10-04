use super::{Cache, Check, Ci, Contract, Files, Phase, runners, unsuppressed};
use anyhow::{Context, Result, ensure};
use std::path::Path;
use yaml_rust2::{Yaml, YamlLoader};

#[derive(Clone, Copy, PartialEq, Eq)]
#[cfg_attr(kani, derive(kani::Arbitrary))]
enum Language {
    Rust,
    Go,
    Swift,
    Dotnet,
    Lean,
    Haskell,
}

#[derive(Clone, Copy, PartialEq, Eq)]
#[cfg_attr(kani, derive(kani::Arbitrary))]
enum Operation {
    Format,
    Analyze,
    Build,
    Test,
}

fn baseline(language: Language) -> &'static [Operation] {
    match language {
        Language::Rust => &[Operation::Format, Operation::Analyze, Operation::Test],
        Language::Go => &[Operation::Analyze, Operation::Test],
        Language::Swift | Language::Dotnet => &[Operation::Build, Operation::Test],
        Language::Lean => &[Operation::Build],
        Language::Haskell => &[Operation::Analyze, Operation::Build, Operation::Test],
    }
}

fn arguments(language: Language, operation: Operation) -> Option<&'static [&'static str]> {
    match (language, operation) {
        (Language::Rust, Operation::Format) => Some(&["cargo", "fmt", "--all", "--", "--check"]),
        (Language::Rust, Operation::Analyze) => Some(&[
            "cargo",
            "clippy",
            "--locked",
            "--all-targets",
            "--all-features",
            "--",
            "-D",
            "warnings",
        ]),
        (Language::Rust, Operation::Test) => Some(&[
            "cargo",
            "test",
            "--locked",
            "--all-targets",
            "--all-features",
        ]),
        (Language::Go, Operation::Analyze) => Some(&["go", "vet", "./..."]),
        (Language::Go, Operation::Test) => Some(&["go", "test", "./..."]),
        (Language::Swift, Operation::Build) => Some(&["swift", "build"]),
        (Language::Swift, Operation::Test) => Some(&["swift", "test"]),
        (Language::Dotnet, Operation::Build) => Some(&["dotnet", "build"]),
        (Language::Dotnet, Operation::Test) => Some(&["dotnet", "test"]),
        (Language::Lean, Operation::Build) => Some(&["lake", "build"]),
        (Language::Haskell, Operation::Analyze) => Some(&["cabal", "check"]),
        (Language::Haskell, Operation::Build) => Some(&["cabal", "build"]),
        (Language::Haskell, Operation::Test) => Some(&["cabal", "test"]),
        _ => None,
    }
}

fn tools(root: &Path, program: &str) -> Result<Vec<String>> {
    let backend = match program {
        "cargo" | "rustc" => "rust",
        "lake" | "lean" => "lean",
        "cabal" => "cabal",
        "ci-policy" => return Ok(Vec::new()),
        other => other,
    };
    let output = super::private_command("mise", root)
        .env("MISE_AUTO_INSTALL", "false")
        .env("MISE_TRUSTED_CONFIG_PATHS", root)
        .args(["ls", "--current", "--json", backend])
        .output()?;
    ensure!(output.status.success(), "cannot discover native {backend}");
    let inventory = crate::json::parse(&output.stdout)?;
    let version = inventory
        .as_array()
        .context("native tool inventory must be an array")?
        .iter()
        .find(|entry| entry["installed"] == true)
        .and_then(|entry| entry["version"].as_str());
    let version = match version {
        Some(version) => version,
        None if backend == "rust" => "1.99.0",
        None => anyhow::bail!("the global native profile must prepare {backend}"),
    };
    Ok(vec![format!("{backend}@{version}")])
}

fn check(root: &Path, id: String, command: Vec<String>, platforms: Vec<String>) -> Result<Check> {
    ensure!(
        super::super::command::permitted(&command),
        "uncovered CI operation: {}",
        command.join(" ")
    );
    let phase = if command.first().is_some_and(|command| command == "cargo")
        && command.get(1).is_some_and(|command| command == "fmt")
    {
        Phase::Commit
    } else {
        Phase::Development
    };
    let mut selected_tools = tools(root, &command[0])?;
    if command[0] == "cargo" {
        let extension = match command.get(1).map(String::as_str) {
            Some("nextest") => Some("cargo:cargo-nextest"),
            Some("deny") => Some("cargo:cargo-deny"),
            Some("llvm-cov") => Some("cargo:cargo-llvm-cov"),
            _ => None,
        };
        if let Some(extension) = extension {
            selected_tools.extend(tools(root, extension)?);
        }
    }
    Ok(Check {
        id,
        tools: selected_tools,
        command,
        phase,
        platforms,
        inputs: vec![".".to_owned()],
        environment: Vec::new(),
        cache: Cache::Content,
        timeout_seconds: if phase == Phase::Commit { 10 } else { 300 },
    })
}

fn words(line: &str) -> Result<Vec<String>> {
    ensure!(
        !line.contains(['\'', '"', '$', '`', '|', '&', ';', '<', '>', '(', ')', '\\']),
        "uncovered CI operation requires a global adapter: {line}"
    );
    let words: Vec<_> = line.split_ascii_whitespace().map(str::to_owned).collect();
    if matches!(words.first().map(String::as_str), Some("mise")) {
        ensure!(
            matches!(words.get(1).map(String::as_str), Some("x" | "exec")),
            "uncovered CI operation: {line}"
        );
        let separator = words
            .iter()
            .position(|word| word == "--")
            .context("uncovered CI operation has no explicit mise command")?;
        ensure!(separator == 2, "CI tool overrides require a global adapter");
        return Ok(words[separator + 1..].to_vec());
    }
    Ok(words)
}

fn scheduled(document: &Yaml) -> bool {
    document["on"].as_hash().is_some_and(|events| {
        !events.is_empty()
            && events
                .keys()
                .all(|event| matches!(event.as_str(), Some("schedule" | "workflow_dispatch")))
    })
}

pub(super) fn discover(root: &Path, files: &Files) -> Result<Contract> {
    let mut checks = Vec::new();
    let platforms = || ["linux", "macos", "windows"].map(str::to_owned).to_vec();
    let languages = [
        (Language::Rust, files.contains_key("Cargo.toml"), "rust"),
        (Language::Go, files.contains_key("go.mod"), "go"),
        (
            Language::Swift,
            files.contains_key("Package.swift"),
            "swift",
        ),
        (
            Language::Dotnet,
            files.keys().any(|path| {
                path.ends_with(".sln")
                    || path.ends_with(".slnx")
                    || path.ends_with(".csproj")
                    || path.ends_with(".fsproj")
            }),
            "dotnet",
        ),
        (
            Language::Lean,
            files.contains_key("lakefile.lean") || files.contains_key("lakefile.toml"),
            "lean",
        ),
        (
            Language::Haskell,
            files.keys().any(|path| path.ends_with(".cabal")),
            "haskell",
        ),
    ];
    for (language, present, name) in languages {
        if !present {
            continue;
        }
        for operation in baseline(language) {
            let command = arguments(language, *operation)
                .context("language baseline has an unsupported operation")?
                .iter()
                .map(|value| (*value).to_owned())
                .collect::<Vec<_>>();
            let suffix = match operation {
                Operation::Format => "fmt",
                Operation::Analyze if language == Language::Rust => "clippy",
                Operation::Analyze => "analyze",
                Operation::Build => "build",
                Operation::Test => "tests",
            };
            checks.push(check(
                root,
                format!("{name}-{suffix}"),
                command,
                platforms(),
            )?);
        }
    }
    let mut ci = Vec::new();
    for (path, source) in files.iter().filter(|(path, _)| {
        crate::verification::file_kind(path, &[]) == crate::verification::FileKind::Workflow
    }) {
        let documents = YamlLoader::load_from_str(std::str::from_utf8(&source.bytes)?)?;
        ensure!(documents.len() == 1, "invalid discovered workflow");
        let document = &documents[0];
        if scheduled(document) {
            println!(
                "{}",
                serde_json::json!({"workflow":path,"outcome":"campaign_not_run"})
            );
            continue;
        }
        ensure!(
            document["env"].is_badvalue() && document["defaults"].is_badvalue(),
            "CI workflow environment requires a global adapter: {path}"
        );
        let jobs = document["jobs"]
            .as_hash()
            .context("CI has no job inventory")?;
        ensure!(!jobs.is_empty(), "CI coverage is empty");
        for (id, job) in jobs {
            let id = id.as_str().context("CI job identity is invalid")?;
            ensure!(
                unsuppressed(job),
                "uncovered CI operation in conditional job {path}::{id}"
            );
            ensure!(
                job["env"].is_badvalue()
                    && job["services"].is_badvalue()
                    && job["container"].is_badvalue()
                    && job["defaults"].is_badvalue(),
                "CI environment requires a global adapter: {path}::{id}"
            );
            let native = runners(job)?.into_iter().collect::<Vec<_>>();
            let steps = job["steps"]
                .as_vec()
                .context("reusable CI requires a global adapter")?;
            let mut ids = Vec::new();
            for (index, step) in steps.iter().enumerate() {
                ensure!(
                    unsuppressed(step),
                    "conditional CI step requires a global adapter"
                );
                if let Some(action) = step["uses"].as_str() {
                    ensure!(
                        action.starts_with("actions/checkout@"),
                        "CI action requires a global adapter: {action}"
                    );
                    continue;
                }
                ensure!(
                    step["working-directory"].is_badvalue() && step["env"].is_badvalue(),
                    "CI step environment requires a global adapter"
                );
                let run = step["run"].as_str().context("CI step has no operation")?;
                for (line_index, line) in run
                    .lines()
                    .filter(|line| !line.trim().is_empty())
                    .enumerate()
                {
                    let command = words(line)?;
                    let identity = format!(
                        "ci-{}-{id}-{index}-{line_index}",
                        Path::new(path)
                            .file_stem()
                            .and_then(|value| value.to_str())
                            .context("invalid workflow filename")?
                    );
                    checks.push(check(root, identity.clone(), command, native.clone())?);
                    ids.push(identity);
                }
            }
            ensure!(
                !ids.is_empty(),
                "CI job has no completed local verification: {path}::{id}"
            );
            ci.push(Ci {
                workflow: Path::new(path)
                    .file_name()
                    .and_then(|value| value.to_str())
                    .context("invalid workflow path")?
                    .to_owned(),
                job: id.to_owned(),
                checks: ids,
            });
        }
    }
    for path in files.keys().filter(|path| super::declared_source(path)) {
        let extension = Path::new(path).extension().and_then(|value| value.to_str());
        let programs: &[&str] = match extension {
            Some("rs") => &["cargo", "rustc"],
            Some("go") => &["go"],
            Some("swift") => &["swift"],
            Some("cs") => &["dotnet"],
            Some("hs") => &["cabal", "ghc"],
            Some("lean") => &["lake", "lean"],
            Some("sh" | "ps1") => continue,
            Some("ts" | "tsx" | "js" | "jsx") => &["bun", "tsc"],
            _ => &[],
        };
        ensure!(
            checks
                .iter()
                .any(|check| programs.contains(&check.command[0].as_str())),
            "uncovered language source requires a global adapter: {path}"
        );
    }
    let mut common = check(
        root,
        "common-source".to_owned(),
        vec!["ci-policy".to_owned(), "source-check".to_owned()],
        platforms(),
    )?;
    common.timeout_seconds = 180;
    common.cache = Cache::Always;
    checks.insert(0, common);
    Ok(Contract {
        version: 1,
        checks,
        ci,
        hosted: Vec::new(),
    })
}

#[cfg(kani)]
mod proofs {
    use super::{Language, Operation, arguments, baseline};

    #[kani::proof]
    #[kani::unwind(10)]
    fn discovered_languages_have_supported_compiler_and_behavior_checks() {
        let language: Language = kani::any();
        let baseline = baseline(language);
        assert!(!baseline.is_empty());
        for operation in baseline {
            let command = arguments(language, *operation).unwrap();
            assert!(!command.is_empty());
            assert!(!matches!(command[1], "publish" | "release" | "sign"));
        }
        if language != Language::Lean {
            assert!(baseline.contains(&Operation::Test));
        }
        assert!(baseline.iter().any(|operation| match language {
            Language::Rust | Language::Go => *operation == Operation::Analyze,
            Language::Swift | Language::Dotnet | Language::Lean | Language::Haskell =>
                *operation == Operation::Build,
        }));
        kani::cover!(language == Language::Rust);
        kani::cover!(language == Language::Lean);
    }
}
