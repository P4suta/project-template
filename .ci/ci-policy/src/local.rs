use std::{
    collections::BTreeSet,
    fs,
    io::Write,
    path::{Component, Path, PathBuf},
    process::{Command, Stdio},
};

use anyhow::{Context, Result, ensure};

use crate::{
    exact_hash,
    verification::{Check, Context as VerificationContext, FileKind, checks_for, file_kind},
    workflow::{Source, actionlint_source, analyze},
};

pub struct PushUpdate {
    local: String,
    remote: Option<String>,
}

fn remote_pattern(remote: &str) -> Result<String> {
    ensure!(
        !remote.is_empty()
            && remote
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || b"._/-".contains(&byte))
            && !remote.starts_with('-'),
        "push remote must have a literal Git remote name"
    );
    Ok(format!("--remotes={remote}"))
}

impl PushUpdate {
    pub fn parse(line: &str) -> Result<Self> {
        let fields: Vec<_> = line.split_whitespace().collect();
        let [local_ref, local, remote_ref, remote] = fields.as_slice() else {
            anyhow::bail!("push ref update must have four fields")
        };
        ensure!(
            (local_ref.starts_with("refs/")
                || *local_ref == "HEAD"
                || exact_hash(local_ref.as_bytes(), 40))
                && remote_ref.starts_with("refs/"),
            "push ref names are invalid"
        );
        ensure!(
            exact_hash(local.as_bytes(), 40) && exact_hash(remote.as_bytes(), 40),
            "push revisions must be exact commit identities"
        );
        ensure!(
            local.bytes().any(|byte| byte != b'0'),
            "ref deletion has no candidate to verify"
        );
        Ok(Self {
            local: local.to_ascii_lowercase(),
            remote: remote
                .bytes()
                .any(|byte| byte != b'0')
                .then(|| remote.to_ascii_lowercase()),
        })
    }

    pub fn local_revision(&self) -> &str {
        &self.local
    }
    pub fn remote_revision(&self) -> Option<&str> {
        self.remote.as_deref()
    }
}

enum Scope<'a> {
    Index,
    Workflows {
        revision: &'a str,
        names: Vec<String>,
    },
    Revision {
        revision: &'a str,
        names: Vec<String>,
        log_options: String,
    },
}

pub struct File {
    pub path: String,
    pub bytes: Vec<u8>,
    pub kind: FileKind,
    checked: bool,
}

fn git(root: &Path, args: &[&str]) -> Result<Vec<u8>> {
    let output = Command::new("git")
        .current_dir(root)
        .args(args)
        .output()
        .context("cannot execute Git")?;
    ensure!(
        output.status.success(),
        "Git could not establish the checked scope: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    Ok(output.stdout)
}

fn paths(bytes: &[u8]) -> Result<Vec<String>> {
    bytes
        .split(|byte| *byte == 0)
        .filter(|name| !name.is_empty())
        .map(|name| {
            let name = std::str::from_utf8(name).context("a checked path is not UTF-8")?;
            ensure!(
                Path::new(name)
                    .components()
                    .all(|part| matches!(part, Component::Normal(_))),
                "a checked path escapes its repository"
            );
            ensure!(
                !name.contains(['\\', ':']),
                "a checked path is not portable"
            );
            Ok(name.to_owned())
        })
        .collect()
}

fn index_names(root: &Path, tree: &str) -> Result<Vec<String>> {
    let head = Command::new("git")
        .current_dir(root)
        .args(["rev-parse", "--verify", "-q", "HEAD"])
        .output()?;
    if head.status.success() {
        let head = std::str::from_utf8(&head.stdout)?.trim();
        paths(&git(
            root,
            &[
                "diff",
                "--name-only",
                "--diff-filter=ACMR",
                "-z",
                head,
                tree,
                "--",
            ],
        )?)
    } else {
        ensure!(
            head.status.code() == Some(1),
            "Git could not establish the index base"
        );
        paths(&git(root, &["ls-tree", "-r", "--name-only", "-z", tree])?)
    }
}

fn support(path: &str) -> bool {
    path.starts_with(".github/workflows/")
        || file_kind(path, &[]) == FileKind::Shell
        || matches!(
            path.rsplit('/').next(),
            Some(
                "_typos.toml"
                    | ".typos.toml"
                    | "typos.toml"
                    | "taplo.toml"
                    | ".taplo.toml"
                    | ".shellcheckrc"
                    | "action.yml"
                    | "action.yaml"
            )
        )
        || matches!(
            path,
            ".github/actionlint.yaml"
                | ".github/actionlint.yml"
                | ".github/zizmor.yml"
                | ".github/zizmor.yaml"
                | "zizmor.yml"
                | "zizmor.yaml"
        )
}

fn snapshot(root: &Path, tree: &str, checked_names: &[String]) -> Result<Vec<File>> {
    let checked_names: BTreeSet<_> = checked_names.iter().cloned().collect();
    let mut names = checked_names.clone();
    names.extend(
        paths(&git(root, &["ls-tree", "-r", "--name-only", "-z", tree])?)?
            .into_iter()
            .filter(|path| support(path)),
    );
    let mut files = Vec::new();
    let mut total_bytes = 0_u64;
    for path in names {
        let metadata = git(root, &["ls-tree", "-z", tree, "--", &path])?;
        let identity = format!("{tree}:{path}");
        let entries: Vec<_> = metadata
            .split(|byte| *byte == 0)
            .filter(|entry| !entry.is_empty())
            .collect();
        if entries.is_empty() {
            continue;
        }
        ensure!(
            entries.len() == 1,
            "{path}: the index is unresolved or incomplete"
        );
        let regular = entries[0].starts_with(b"100644 ") || entries[0].starts_with(b"100755 ");
        let link = entries[0].starts_with(b"120000 ");
        ensure!(
            regular || link,
            "{path}: a checked file must be a regular tracked file"
        );
        let size = git(root, &["cat-file", "-s", &identity])?;
        let size: u64 = std::str::from_utf8(&size)?
            .trim()
            .parse()
            .context("Git blob size is invalid")?;
        total_bytes = total_bytes
            .checked_add(size)
            .context("snapshot size overflow")?;
        ensure!(
            total_bytes <= 64 * 1024 * 1024,
            "checked files exceed the 64 MiB local verification budget"
        );
        let bytes = git(root, &["show", &identity])?;
        let kind = if link {
            FileKind::Other
        } else {
            file_kind(&path, &bytes)
        };
        ensure!(
            !link || !path.starts_with(".github/"),
            "workflow links cannot establish checked source coverage"
        );
        ensure!(
            !link || !support(&path),
            "verification configuration must be a regular tracked file"
        );
        let checked = checked_names.contains(&path);
        files.push(File {
            path,
            bytes,
            kind,
            checked,
        });
    }
    Ok(files)
}

pub fn execute(command: &mut Command, input: Option<&[u8]>) -> Result<()> {
    command.stdin(if input.is_some() {
        Stdio::piped()
    } else {
        Stdio::null()
    });
    let mut child = command
        .spawn()
        .context("a required verification tool could not start")?;
    let written = match input {
        Some(bytes) => child
            .stdin
            .take()
            .context("verification input is unavailable")
            .and_then(|mut stdin| {
                stdin
                    .write_all(bytes)
                    .context("verification input could not be delivered")
            }),
        None => Ok(()),
    };
    let waited = child
        .wait()
        .context("a required verification tool could not be awaited");
    written?;
    let status = waited?;
    ensure!(
        status.success(),
        "a required verification tool did not succeed ({status})"
    );
    Ok(())
}

fn selected(files: &[File], kind: FileKind, directory: &Path) -> Vec<PathBuf> {
    files
        .iter()
        .filter(|file| file.checked && file.kind == kind)
        .map(|file| directory.join(&file.path))
        .collect()
}

pub fn verify_index(root: &Path) -> Result<()> {
    verify(root, &Scope::Index)
}

pub fn verify_skill_maintenance() -> Result<()> {
    let program = crate::hooks::home()?
        .join(".local/bin")
        .join(if cfg!(windows) {
            "skill-ops.exe"
        } else {
            "skill-ops"
        });
    execute(Command::new(program).arg("check"), None)
        .context("shared skill maintenance is incomplete")
}

pub fn verify_workflows(root: &Path) -> Result<()> {
    let revision = git(root, &["rev-parse", "HEAD^{commit}"])?;
    let revision = std::str::from_utf8(&revision)?.trim();
    ensure!(
        exact_hash(revision.as_bytes(), 40),
        "workflow revision is invalid"
    );
    let names: Vec<_> = paths(&git(
        root,
        &["ls-tree", "-r", "--name-only", "-z", revision],
    )?)?
    .into_iter()
    .filter(|path| matches!(file_kind(path, &[]), FileKind::Workflow | FileKind::Action))
    .collect();
    ensure!(
        names
            .iter()
            .any(|path| file_kind(path, &[]) == FileKind::Workflow),
        "workflow coverage is empty"
    );
    verify(root, &Scope::Workflows { revision, names })
}

pub fn verify_push(root: &Path, remote_name: &str, input: &str) -> Result<()> {
    ensure!(
        input.len() <= 64 * 1024,
        "push ref input exceeds the verification budget"
    );
    let remote_pattern = remote_pattern(remote_name)?;
    if input.trim().is_empty() {
        return verify_skill_maintenance();
    }
    for line in input.lines().filter(|line| !line.trim().is_empty()) {
        let update = PushUpdate::parse(line)?;
        let revision = update.local_revision();
        let mut names = BTreeSet::new();
        let log_options = if let Some(previous) = update.remote_revision() {
            names.extend(paths(&git(
                root,
                &[
                    "diff",
                    "--name-only",
                    "--diff-filter=ACMR",
                    "-z",
                    previous,
                    revision,
                    "--",
                ],
            )?)?);
            format!("{previous}..{revision}")
        } else {
            let commits = git(root, &["rev-list", revision, "--not", &remote_pattern])?;
            let commits = std::str::from_utf8(&commits)?.lines().collect::<Vec<_>>();
            ensure!(
                commits.len() <= 1000,
                "new branch exceeds the 1000-commit verification budget"
            );
            for commit in commits {
                ensure!(
                    exact_hash(commit.as_bytes(), 40),
                    "Git returned an invalid commit identity"
                );
                names.extend(paths(&git(
                    root,
                    &[
                        "diff-tree",
                        "--root",
                        "--no-commit-id",
                        "--name-only",
                        "--diff-filter=ACMR",
                        "-r",
                        "-m",
                        "-z",
                        commit,
                        "--",
                    ],
                )?)?);
            }
            format!("{revision} --not --remotes={remote_name}")
        };
        verify(
            root,
            &Scope::Revision {
                revision,
                names: names.into_iter().collect(),
                log_options,
            },
        )?;
    }
    Ok(())
}

fn verify(root: &Path, scope: &Scope<'_>) -> Result<()> {
    let repository = git(root, &["rev-parse", "--show-toplevel"])?;
    let root = PathBuf::from(std::str::from_utf8(&repository)?.trim())
        .canonicalize()
        .context("repository is unavailable")?;
    let initial = if matches!(scope, Scope::Index) {
        Some(git(&root, &["write-tree"])?)
    } else {
        None
    };
    let tree = match scope {
        Scope::Index => {
            std::str::from_utf8(initial.as_deref().context("index identity is missing")?)?.trim()
        }
        Scope::Revision { revision, .. } | Scope::Workflows { revision, .. } => revision,
    };
    let names = match scope {
        Scope::Index => index_names(&root, tree)?,
        Scope::Revision { names, .. } | Scope::Workflows { names, .. } => names.clone(),
    };
    let files = snapshot(&root, tree, &names)?;
    let context = if matches!(scope, Scope::Workflows { .. }) {
        VerificationContext::Repository
    } else {
        VerificationContext::Personal
    };
    let plan = checks_for(
        context,
        &files
            .iter()
            .filter(|file| file.checked)
            .map(|file| file.kind)
            .collect::<Vec<_>>(),
    );
    let temporary = tempfile::Builder::new()
        .prefix("ci-policy-index-")
        .tempdir()?;
    let candidate = temporary.path().join("candidate");
    fs::create_dir(&candidate)?;
    for file in &files {
        let path = candidate.join(&file.path);
        fs::create_dir_all(path.parent().context("snapshot path has no parent")?)?;
        fs::write(path, &file.bytes)?;
    }
    fs::create_dir(candidate.join(".git"))?;
    let secret_config = temporary.path().join("default-gitleaks.toml");
    fs::write(&secret_config, "[extend]\nuseDefault = true\n")?;
    let verification = (|| -> Result<()> {
        for check in plan {
            match check {
                Check::Skills => {
                    verify_skill_maintenance()?;
                }
                Check::Spelling => {
                    let mut command = crate::tools::command(&candidate, "typos")?;
                    command.args(["--threads", "2", "--"]).args(
                        files
                            .iter()
                            .filter(|file| file.checked)
                            .map(|file| &file.path),
                    );
                    execute(&mut command, None).context("spelling check failed")?;
                }
                Check::Secrets => {
                    let mut command = crate::tools::command(&candidate, "gitleaks")?;
                    command
                        .env_remove("GITLEAKS_CONFIG")
                        .env_remove("GITLEAKS_CONFIG_TOML")
                        .args([
                            "--redact",
                            "--no-banner",
                            "--ignore-gitleaks-allow",
                            "--timeout",
                            "120",
                            "--config",
                        ])
                        .arg(&secret_config)
                        .arg("--gitleaks-ignore-path")
                        .arg(temporary.path().join("absent-ignore-file"));
                    match scope {
                        Scope::Index | Scope::Workflows { .. } => {
                            command.arg("dir").arg(&candidate);
                        }
                        Scope::Revision { log_options, .. } => {
                            command.args(["git", "--log-opts", log_options]).arg(&root);
                        }
                    }
                    execute(&mut command, None).context("secret scan failed")?;
                }
                Check::Shell => {
                    let mut command = crate::tools::command(&candidate, "shellcheck")?;
                    command
                        .args(["--severity=style", "--enable=all", "--"])
                        .args(selected(&files, FileKind::Shell, &candidate));
                    execute(&mut command, None).context("shell check failed")?;
                }
                Check::Powershell => {
                    for path in selected(&files, FileKind::Powershell, &candidate) {
                        let mut command = crate::tools::command(&candidate, "pwsh")?;
                        command.env("CI_POLICY_INPUT", &path).args(["-NoProfile", "-NonInteractive", "-Command", "$ErrorActionPreference = 'Stop'; Import-Module PSScriptAnalyzer -RequiredVersion 1.25.0 -ErrorAction Stop; Invoke-ScriptAnalyzer -Path $env:CI_POLICY_INPUT -EnableExit"]);
                        execute(&mut command, None).context("PowerShell check failed")?;
                    }
                }
                Check::Toml => {
                    let mut command = crate::tools::command(&candidate, "taplo")?;
                    command
                        .arg("lint")
                        .args(selected(&files, FileKind::Toml, &candidate));
                    execute(&mut command, None).context("TOML check failed")?;
                }
                Check::Json => {
                    for file in files
                        .iter()
                        .filter(|file| file.checked && file.kind == FileKind::Json)
                    {
                        crate::json::parse(&file.bytes)
                            .with_context(|| format!("{}: invalid JSON", file.path))?;
                    }
                }
                Check::Yaml => {
                    for file in files.iter().filter(|file| {
                        file.checked
                            && matches!(
                                file.kind,
                                FileKind::Yaml | FileKind::Workflow | FileKind::Action
                            )
                    }) {
                        let text = std::str::from_utf8(&file.bytes)?;
                        ensure!(
                            !yaml_rust2::YamlLoader::load_from_str(text)?.is_empty(),
                            "{}: YAML coverage is empty",
                            file.path
                        );
                    }
                }
                Check::Workflow => {
                    for file in files
                        .iter()
                        .filter(|file| file.checked && file.kind == FileKind::Workflow)
                    {
                        let source = Source {
                            name: file.path.clone(),
                            text: std::str::from_utf8(&file.bytes)?.to_owned(),
                        };
                        let summary = analyze("checkout", &source)?;
                        for finding in &summary.findings {
                            eprintln!(
                                "{}:{}: {:?}: {}",
                                finding.workflow,
                                finding.job.as_deref().unwrap_or("workflow"),
                                finding.rule,
                                finding.detail
                            );
                        }
                        ensure!(summary.findings.is_empty(), "workflow policy failed");
                    }
                }
                Check::Actionlint => {
                    for file in files
                        .iter()
                        .filter(|file| file.checked && file.kind == FileKind::Workflow)
                    {
                        let mut command = crate::tools::command(&candidate, "actionlint")?;
                        command.args(["-stdin-filename", &file.path, "-"]);
                        let source = Source {
                            name: file.path.clone(),
                            text: std::str::from_utf8(&file.bytes)?.to_owned(),
                        };
                        let projected = actionlint_source(&source)?;
                        execute(&mut command, Some(projected.as_bytes()))
                            .context("workflow syntax check failed")?;
                    }
                }
                Check::Zizmor => {
                    let mut command = crate::tools::command(&candidate, "zizmor")?;
                    command
                        .args([
                            "--no-progress",
                            "--no-online-audits",
                            "--persona",
                            "auditor",
                            "--min-severity",
                            "low",
                        ])
                        .args(
                            files
                                .iter()
                                .filter(|file| {
                                    file.checked
                                        && matches!(
                                            file.kind,
                                            FileKind::Workflow | FileKind::Action
                                        )
                                })
                                .map(|file| candidate.join(&file.path)),
                        );
                    execute(&mut command, None).context("workflow security check failed")?;
                }
            }
        }
        if let Some(initial) = &initial {
            ensure!(
                initial == &git(&root, &["write-tree"])?,
                "the index changed during verification; verify the new content"
            );
        }
        Ok(())
    })();
    let cleanup = temporary
        .close()
        .context("verification snapshot could not be removed");
    if verification.is_err()
        && let Err(error) = &cleanup
    {
        eprintln!("{error:#}");
    }
    verification?;
    cleanup?;
    println!(
        "Verified {} candidate files with the common development policy",
        files.iter().filter(|file| file.checked).count()
    );
    Ok(())
}

pub fn indexed_files(root: &Path) -> Result<Vec<File>> {
    let tree = git(root, &["write-tree"])?;
    let tree = std::str::from_utf8(&tree)?.trim();
    snapshot(root, tree, &index_names(root, tree)?)
}

pub fn actionlint(root: &Path) -> Result<()> {
    let mut count = 0;
    for entry in fs::read_dir(root.join(".github/workflows"))? {
        let entry = entry?;
        let path = entry.path();
        ensure!(
            !entry.file_type()?.is_symlink(),
            "workflow source must be a regular file"
        );
        if entry.file_type()?.is_file()
            && matches!(
                path.extension().and_then(|extension| extension.to_str()),
                Some("yml" | "yaml")
            )
        {
            let source = Source {
                name: path
                    .to_str()
                    .context("workflow path is not UTF-8")?
                    .to_owned(),
                text: fs::read_to_string(&path)?,
            };
            let projected = actionlint_source(&source)?;
            execute(
                crate::tools::command(root, "actionlint")?.args([
                    "-stdin-filename",
                    &source.name,
                    "-",
                ]),
                Some(projected.as_bytes()),
            )?;
            count += 1;
        }
    }
    ensure!(count != 0, "workflow syntax coverage is empty");
    Ok(())
}
