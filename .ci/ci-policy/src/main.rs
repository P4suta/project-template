use std::{
    collections::BTreeMap,
    fs,
    io::{Read, Write},
    path::{Path, PathBuf},
};

use anyhow::{Context, Result, ensure};
use clap::{Arg, Command, builder::PathBufValueParser};
use serde::Deserialize;

use ci_policy::{
    Conclusion, gate,
    workflow::{Repository, analyze},
};

struct Cli {
    command: Action,
}

enum Action {
    ProjectCheck {
        root: PathBuf,
        phase: ci_policy::project::protocol::Phase,
        suite: String,
        cache: Option<PathBuf>,
        index: bool,
        revision: Option<String>,
    },
    TemplatePatch {
        root: PathBuf,
        rendered: PathBuf,
        engine: PathBuf,
        output: PathBuf,
    },
    Audit {
        inventory: PathBuf,
        output: PathBuf,
    },
    Check {
        root: PathBuf,
    },
    Export {
        inventory: PathBuf,
        output: PathBuf,
    },
    Gate {
        needs: String,
        require: Vec<String>,
    },
    VerifyIndex {
        root: PathBuf,
    },
    PrePush {
        root: PathBuf,
        remote: String,
        expected: Option<String>,
    },
    Activate {
        root: PathBuf,
        expected: String,
    },
    Doctor {
        root: PathBuf,
        expected: String,
    },
    CommitHook {
        root: PathBuf,
        expected: String,
        guard: bool,
    },
    PushHook {
        root: PathBuf,
        expected: String,
        remote: String,
        url: String,
    },
    Identity,
    Actionlint {
        root: PathBuf,
    },
    InstallTools {
        workflow_only: bool,
    },
    Prove {
        manifest: PathBuf,
    },
    ProveSource {
        source: PathBuf,
        required: Vec<String>,
        counterexample: String,
    },
}

impl Cli {
    fn parse() -> Result<Self> {
        let path = |name: &'static str| {
            Arg::new(name)
                .long(name)
                .required(true)
                .value_parser(PathBufValueParser::new())
        };
        let root_argument = || {
            Arg::new("root")
                .long("root")
                .default_value(".")
                .value_parser(PathBufValueParser::new())
        };
        let expected_argument = || {
            Arg::new("expected-revision")
                .long("expected-revision")
                .required(true)
        };
        let command = Command::new("ci-policy")
            .version(env!("CARGO_PKG_VERSION"))
            .about("Validate reproducible CI configuration and fail-closed required jobs")
            .subcommand_required(true)
            .subcommand(
                Command::new("project-check")
                    .arg(root_argument())
                    .arg(
                        Arg::new("phase")
                            .long("phase")
                            .default_value("development")
                            .value_parser(["commit", "development"]),
                    )
                    .arg(Arg::new("suite").long("suite").default_value("local"))
                    .arg(
                        Arg::new("cache-directory")
                            .long("cache-directory")
                            .value_parser(PathBufValueParser::new()),
                    )
                    .arg(
                        Arg::new("index")
                            .long("index")
                            .action(clap::ArgAction::SetTrue)
                            .conflicts_with("revision"),
                    )
                    .arg(Arg::new("revision").long("revision")),
            )
            .subcommand(
                Command::new("template-patch")
                    .arg(root_argument())
                    .arg(path("rendered"))
                    .arg(path("engine"))
                    .arg(path("output")),
            )
            .subcommand(Command::new("identity"))
            .subcommand(
                Command::new("activate")
                    .arg(root_argument())
                    .arg(expected_argument()),
            )
            .subcommand(
                Command::new("doctor")
                    .arg(root_argument())
                    .arg(expected_argument()),
            )
            .subcommand(
                Command::new("commit-hook")
                    .arg(root_argument())
                    .arg(expected_argument())
                    .arg(
                        Arg::new("guard")
                            .long("guard")
                            .action(clap::ArgAction::SetTrue),
                    ),
            )
            .subcommand(
                Command::new("push-hook")
                    .arg(root_argument())
                    .arg(expected_argument())
                    .arg(Arg::new("remote").required(true))
                    .arg(Arg::new("url").required(true)),
            )
            .subcommand(
                Command::new("actionlint").arg(
                    Arg::new("root")
                        .default_value(".")
                        .value_parser(PathBufValueParser::new()),
                ),
            )
            .subcommand(
                Command::new("install-tools").arg(
                    Arg::new("workflow-only")
                        .long("workflow-only")
                        .action(clap::ArgAction::SetTrue),
                ),
            )
            .subcommand(Command::new("prove").arg(path("manifest-path")))
            .subcommand(
                Command::new("prove-source")
                    .arg(path("source"))
                    .arg(Arg::new("require-json").long("require-json").required(true))
                    .arg(
                        Arg::new("counterexample")
                            .long("counterexample")
                            .required(true),
                    ),
            )
            .subcommand(
                Command::new("audit")
                    .arg(path("inventory"))
                    .arg(path("output")),
            )
            .subcommand(
                Command::new("export")
                    .arg(path("inventory"))
                    .arg(path("output")),
            )
            .subcommand(
                Command::new("check").arg(
                    Arg::new("root")
                        .default_value(".")
                        .value_parser(PathBufValueParser::new()),
                ),
            )
            .subcommand(
                Command::new("verify-index").arg(
                    Arg::new("root")
                        .default_value(".")
                        .value_parser(PathBufValueParser::new()),
                ),
            )
            .subcommand(
                Command::new("pre-push")
                    .arg(Arg::new("remote").required(true))
                    .arg(Arg::new("expected-revision").long("expected-revision"))
                    .arg(
                        Arg::new("root")
                            .long("root")
                            .default_value(".")
                            .value_parser(PathBufValueParser::new()),
                    ),
            )
            .subcommand(
                Command::new("gate")
                    .arg(Arg::new("needs").long("needs").required(true))
                    .arg(
                        Arg::new("require")
                            .long("require")
                            .required_unless_present("require-json")
                            .num_args(1..),
                    )
                    .arg(
                        Arg::new("require-json")
                            .long("require-json")
                            .conflicts_with("require"),
                    ),
            )
            .get_matches();
        let (name, matches) = command.subcommand().context("a command is required")?;
        let get_path = |name| {
            matches
                .get_one::<PathBuf>(name)
                .cloned()
                .context("a path is required")
        };
        let get_string = |name| {
            matches
                .get_one::<String>(name)
                .cloned()
                .context("a required argument is missing")
        };
        let command = match name {
            "project-check" => Action::ProjectCheck {
                root: get_path("root")?,
                phase: if get_string("phase")? == "commit" {
                    ci_policy::project::protocol::Phase::Commit
                } else {
                    ci_policy::project::protocol::Phase::Development
                },
                suite: get_string("suite")?,
                cache: matches.get_one::<PathBuf>("cache-directory").cloned(),
                index: matches.get_flag("index"),
                revision: matches.get_one::<String>("revision").cloned(),
            },
            "template-patch" => Action::TemplatePatch {
                root: get_path("root")?,
                rendered: get_path("rendered")?,
                engine: get_path("engine")?,
                output: get_path("output")?,
            },
            "identity" => Action::Identity,
            "activate" => Action::Activate {
                root: get_path("root")?,
                expected: get_string("expected-revision")?,
            },
            "doctor" => Action::Doctor {
                root: get_path("root")?,
                expected: get_string("expected-revision")?,
            },
            "commit-hook" => Action::CommitHook {
                root: get_path("root")?,
                expected: get_string("expected-revision")?,
                guard: matches.get_flag("guard"),
            },
            "push-hook" => Action::PushHook {
                root: get_path("root")?,
                expected: get_string("expected-revision")?,
                remote: get_string("remote")?,
                url: get_string("url")?,
            },
            "actionlint" => Action::Actionlint {
                root: get_path("root")?,
            },
            "install-tools" => Action::InstallTools {
                workflow_only: matches.get_flag("workflow-only"),
            },
            "prove" => Action::Prove {
                manifest: get_path("manifest-path")?,
            },
            "prove-source" => Action::ProveSource {
                source: get_path("source")?,
                required: serde_json::from_value(ci_policy::json::parse(
                    get_string("require-json")?.as_bytes(),
                )?)?,
                counterexample: get_string("counterexample")?,
            },
            "audit" => Action::Audit {
                inventory: get_path("inventory")?,
                output: get_path("output")?,
            },
            "export" => Action::Export {
                inventory: get_path("inventory")?,
                output: get_path("output")?,
            },
            "check" => Action::Check {
                root: get_path("root")?,
            },
            "verify-index" => Action::VerifyIndex {
                root: get_path("root")?,
            },
            "pre-push" => Action::PrePush {
                root: get_path("root")?,
                remote: matches
                    .get_one::<String>("remote")
                    .cloned()
                    .context("push remote is required")?,
                expected: matches.get_one::<String>("expected-revision").cloned(),
            },
            "gate" => Action::Gate {
                needs: matches
                    .get_one::<String>("needs")
                    .cloned()
                    .context("needs is required")?,
                require: if let Some(value) = matches.get_one::<String>("require-json") {
                    serde_json::from_value(ci_policy::json::parse(value.as_bytes())?)?
                } else {
                    matches
                        .get_many::<String>("require")
                        .context("required jobs are missing")?
                        .cloned()
                        .collect()
                },
            },
            _ => anyhow::bail!("unsupported command"),
        };
        Ok(Self { command })
    }
}

fn inventory(path: &Path) -> Result<Vec<Repository>> {
    let repositories: Vec<Repository> = serde_json::from_slice(&fs::read(path)?)?;
    ensure!(!repositories.is_empty(), "repository inventory is empty");
    for repository in &repositories {
        let segments: Vec<_> = repository.repository.split('/').collect();
        ensure!(
            segments.len() == 2
                && segments.iter().all(|segment| !segment.is_empty()
                    && segment
                        .bytes()
                        .all(|byte| byte.is_ascii_alphanumeric() || b"._-".contains(&byte))
                    && *segment != "."
                    && *segment != ".."),
            "invalid repository identity"
        );
        ensure!(
            !repository.workflows.is_empty(),
            "{} has no workflow coverage",
            repository.repository
        );
        for source in &repository.workflows {
            ensure!(
                !source.name.contains(['/', '\\']) && source.name.ends_with(".yml")
                    || !source.name.contains(['/', '\\']) && source.name.ends_with(".yaml"),
                "invalid workflow filename"
            );
        }
    }
    Ok(repositories)
}

#[derive(Deserialize)]
struct Job {
    result: String,
}

fn run() -> Result<()> {
    match Cli::parse()?.command {
        Action::ProjectCheck {
            root,
            phase,
            suite,
            cache,
            index,
            revision,
        } => {
            let scope = if let Some(revision) = revision.as_deref() {
                ci_policy::project::Scope::Revision(ci_policy::project::Revision::parse(revision)?)
            } else if index {
                ci_policy::project::Scope::Index
            } else {
                ci_policy::project::Scope::Working
            };
            ci_policy::project::run(&root, scope, phase, &suite, cache.as_deref())?;
        }
        Action::TemplatePatch {
            root,
            rendered,
            engine,
            output,
        } => ci_policy::handoff::patch(&root, &rendered, &engine, &output)?,
        Action::Identity => println!("{}", ci_policy::REVISION),
        Action::Activate { root, expected } => ci_policy::hooks::activate(&root, &expected)?,
        Action::Doctor { root, expected } => ci_policy::hooks::doctor(&root, &expected)?,
        Action::CommitHook {
            root,
            expected,
            guard,
        } => ci_policy::hooks::commit(&root, &expected, guard)?,
        Action::PushHook {
            root,
            expected,
            remote,
            url,
        } => {
            let mut input = String::new();
            std::io::stdin()
                .take(64 * 1024 + 1)
                .read_to_string(&mut input)?;
            ci_policy::hooks::push(&root, &expected, &remote, &url, &input)?;
        }
        Action::Actionlint { root } => ci_policy::local::actionlint(&root)?,
        Action::InstallTools { workflow_only } => ci_policy::tools::install(workflow_only)?,
        Action::Prove { manifest } => ci_policy::proof::prove(&manifest)?,
        Action::ProveSource {
            source,
            required,
            counterexample,
        } => ci_policy::proof::prove_source(&source, &required, &counterexample)?,
        Action::VerifyIndex { root } => ci_policy::local::verify_index(&root)?,
        Action::PrePush {
            root,
            remote,
            expected,
        } => {
            if let Some(expected) = expected {
                ci_policy::hooks::doctor(&root, &expected)?;
            }
            ci_policy::hooks::check_push_hold()?;
            let mut input = String::new();
            std::io::stdin()
                .take(64 * 1024 + 1)
                .read_to_string(&mut input)?;
            ci_policy::local::verify_push(&root, &remote, &input)?;
        }
        Action::Audit {
            inventory: path,
            output,
        } => {
            let mut summaries = Vec::new();
            for repository in inventory(&path)? {
                for source in &repository.workflows {
                    summaries.push(analyze(&repository.repository, source)?);
                }
            }
            let count: usize = summaries.iter().map(|summary| summary.findings.len()).sum();
            let mut file = fs::OpenOptions::new()
                .write(true)
                .create_new(true)
                .open(output)?;
            serde_json::to_writer_pretty(&mut file, &summaries)?;
            file.write_all(b"\n")?;
            println!("Audited {} workflows; {count} findings", summaries.len());
        }
        Action::Check { root } => {
            ci_policy::local::verify_workflows(&root)?;
        }
        Action::Export {
            inventory: path,
            output,
        } => {
            let repositories = inventory(&path)?;
            fs::create_dir(&output).context("export needs a fresh output directory")?;
            for repository in repositories {
                let directory = output.join(repository.repository).join(".github/workflows");
                fs::create_dir_all(&directory)?;
                for source in repository.workflows {
                    let mut file = fs::OpenOptions::new()
                        .create_new(true)
                        .write(true)
                        .open(directory.join(source.name))?;
                    file.write_all(source.text.as_bytes())?;
                }
            }
        }
        Action::Gate { needs, require } => {
            let jobs: BTreeMap<String, Job> =
                serde_json::from_value(ci_policy::json::parse(needs.as_bytes())?)
                    .context("invalid needs result")?;
            let mut conclusions = Vec::new();
            for name in require {
                let job = jobs
                    .get(&name)
                    .with_context(|| format!("required job {name} is absent"))?;
                let conclusion = match job.result.as_str() {
                    "success" => Conclusion::Success,
                    "failure" => Conclusion::Failure,
                    "cancelled" => Conclusion::Cancelled,
                    "skipped" => Conclusion::Skipped,
                    _ => anyhow::bail!("job {name} has an unknown result"),
                };
                conclusions.push(conclusion);
            }
            ensure!(gate(&conclusions), "a required job did not succeed");
            println!("Every required job succeeded");
        }
    }
    Ok(())
}

fn main() {
    if let Err(error) = run() {
        eprintln!("error: {error:#}");
        std::process::exit(1);
    }
}
