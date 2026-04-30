//! `tmpl` binary entry point. Dispatches sub-commands; each sub-command
//! is a thin shell over the library API in [`tmpl::template`].

#![deny(missing_docs)]

use std::collections::BTreeMap;
use std::fmt::Write as _;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::process::{Command as ProcessCommand, ExitCode};

use clap::{Parser, Subcommand};
use miette::IntoDiagnostic;

use tmpl::Context;
use tmpl::ctx::ProjectInfo;
use tmpl::layer::LayerName;
use tmpl::state::{ContentHash, DriftReport, State, detect_drift, merkle_root};
use tmpl::template::{Loaded, Template};

/// CLI surface.
#[derive(Debug, Parser)]
#[command(
    name = "tmpl",
    version,
    about = "Layer-DAG template engine for project-template",
    long_about = None,
)]
struct Cli {
    /// Template root (directory containing `manifest.toml`). Defaults
    /// to `.template` relative to the current working directory.
    #[arg(long, global = true, default_value = ".template")]
    template_root: PathBuf,
    /// Destination directory for `apply` / `add`. Defaults to the
    /// current working directory.
    #[arg(long, global = true, default_value = ".")]
    dest: PathBuf,
    /// Sub-command.
    #[command(subcommand)]
    command: Command,
}

/// Sub-commands.
#[derive(Debug, Subcommand)]
enum Command {
    /// Apply a layer selection to the destination, writing the
    /// rendered files and recording state.
    Apply {
        /// Comma-separated layer names. Defaults to the manifest's
        /// `default_selection`.
        #[arg(long, value_delimiter = ',')]
        layers: Vec<String>,
        /// Repository name for the render context.
        #[arg(long)]
        project_name: String,
        /// Repository owner / GitHub login.
        #[arg(long)]
        project_owner: String,
        /// Optional one-line description.
        #[arg(long, default_value = "")]
        project_description: String,
    },
    /// Add a single layer on top of an already-applied state. Re-runs
    /// the resolution + render pipeline with the existing layer set
    /// extended by the new layer; existing files are re-rendered so
    /// they stay coherent with the updated capability graph.
    Add {
        /// Layer to add.
        layer: String,
        /// Repository name for the render context.
        #[arg(long)]
        project_name: String,
        /// Repository owner / GitHub login.
        #[arg(long)]
        project_owner: String,
        /// Optional one-line description.
        #[arg(long, default_value = "")]
        project_description: String,
        /// Overwrite locally-edited files. Without `--force`, the
        /// command refuses to proceed if any previously-rendered file
        /// has been modified on disk.
        #[arg(long, default_value_t = false)]
        force: bool,
    },
    /// Remove an applied layer. Deletes the files the layer
    /// contributed and updates state. Refuses if any of those files
    /// have been modified locally; pass `--force` to delete anyway.
    Remove {
        /// Layer to remove.
        layer: String,
        /// Delete files even if they have been modified locally.
        #[arg(long, default_value_t = false)]
        force: bool,
    },
    /// Run manifest + layer DAG soundness checks. Used by the engine's
    /// own CI as well as `just verify-template`.
    Verify,
    /// Print the current applied state, if any.
    Status,
    /// Delete `.template/` and graduate from the engine. Idempotent —
    /// re-running on a sealed repo is a structured no-op.
    Seal,
    /// Generate a new project from a remote GitHub template via the
    /// `gh` CLI. Equivalent to `gh repo create --template owner/repo
    /// dest --clone` plus an automatic `bash .template/bootstrap.sh`.
    New {
        /// Source template, e.g. `gh:P4suta/project-template`.
        source: String,
        /// Destination repository name (becomes the local directory).
        dest: String,
        /// Create as a public repo (default: private).
        #[arg(long, default_value_t = false)]
        public: bool,
    },
}

/// Bundled inputs for [`apply`]. Grouped into a struct so the function
/// signature stays under the four-argument cap enforced by clippy.toml.
struct ApplyInvocation<'a> {
    template_root: &'a Path,
    dest: &'a Path,
    layers: &'a [String],
    project: ProjectFacts<'a>,
}

/// Bundled inputs for [`add`].
struct AddInvocation<'a> {
    template_root: &'a Path,
    dest: &'a Path,
    new_layer: &'a str,
    project: ProjectFacts<'a>,
    force: bool,
}

/// Repository facts used to build the [`Context`]. Bundled to keep
/// signatures narrow and to mirror the manifest's variable shape.
struct ProjectFacts<'a> {
    name: &'a str,
    owner: &'a str,
    description: &'a str,
}

fn main() -> ExitCode {
    if let Err(e) = run() {
        eprintln!("{e:?}");
        return ExitCode::from(1);
    }
    ExitCode::from(0)
}

fn run() -> miette::Result<()> {
    let cli = Cli::parse();
    match cli.command {
        Command::Apply {
            layers,
            project_name,
            project_owner,
            project_description,
        } => apply(&ApplyInvocation {
            template_root: &cli.template_root,
            dest: &cli.dest,
            layers: &layers,
            project: ProjectFacts {
                name: &project_name,
                owner: &project_owner,
                description: &project_description,
            },
        }),
        Command::Add {
            layer,
            project_name,
            project_owner,
            project_description,
            force,
        } => add(&AddInvocation {
            template_root: &cli.template_root,
            dest: &cli.dest,
            new_layer: &layer,
            project: ProjectFacts {
                name: &project_name,
                owner: &project_owner,
                description: &project_description,
            },
            force,
        }),
        Command::Remove { layer, force } => remove(&cli.dest, &layer, force),
        Command::Verify => verify(&cli.template_root),
        Command::Status => status(&cli.dest),
        Command::Seal => seal(&cli.template_root),
        Command::New {
            source,
            dest,
            public,
        } => new_command(&source, &dest, public),
    }
}

fn apply(invocation: &ApplyInvocation<'_>) -> miette::Result<()> {
    let parsed_layers = parse_layer_names(invocation.layers)?;
    let ctx = build_context(&invocation.project);
    let applied = Template::<Loaded>::load(invocation.template_root)
        .into_diagnostic()?
        .validate()
        .into_diagnostic()?
        .resolve(&parsed_layers, ctx)
        .into_diagnostic()?
        .render()
        .into_diagnostic()?
        .apply(invocation.dest)
        .into_diagnostic()?;
    println!("merkle_root = {}", applied.merkle_root().to_hex());
    println!("layers      = {}", applied.state().applied.len());
    Ok(())
}

fn add(invocation: &AddInvocation<'_>) -> miette::Result<()> {
    let state_path = invocation.dest.join(".template").join("state.toml");
    let existing = State::load(&state_path).into_diagnostic()?;

    let new_layer = LayerName::new(invocation.new_layer)
        .map_err(|e| miette::miette!(code = "tmpl::cli", "invalid layer name: {e}"))?;
    if existing.applied.contains_key(&new_layer) {
        println!("layer '{new_layer}' is already applied — nothing to do");
        return Ok(());
    }

    // Drift check: every previously-applied layer's files must still
    // match the recorded hashes, otherwise we'd silently overwrite
    // user edits when re-applying.
    if !invocation.force {
        let drift = collect_drift(invocation.dest, &existing).into_diagnostic()?;
        if !drift.is_empty() {
            return Err(drift_error(&drift, "tmpl::add::drift"));
        }
    }

    let mut selection: Vec<LayerName> = existing.applied.keys().cloned().collect();
    selection.push(new_layer);

    let ctx = build_context(&invocation.project);
    let applied = Template::<Loaded>::load(invocation.template_root)
        .into_diagnostic()?
        .validate()
        .into_diagnostic()?
        .resolve(&selection, ctx)
        .into_diagnostic()?
        .render()
        .into_diagnostic()?
        .apply(invocation.dest)
        .into_diagnostic()?;
    println!("merkle_root = {}", applied.merkle_root().to_hex());
    println!("layers      = {}", applied.state().applied.len());
    Ok(())
}

fn remove(dest: &Path, layer: &str, force: bool) -> miette::Result<()> {
    let state_path = dest.join(".template").join("state.toml");
    let mut state = State::load(&state_path).into_diagnostic()?;

    let target = LayerName::new(layer)
        .map_err(|e| miette::miette!(code = "tmpl::cli", "invalid layer name: {e}"))?;
    let entry = state.applied.get(&target).ok_or_else(|| {
        miette::miette!(
            code = "tmpl::remove::not-applied",
            "layer '{target}' is not in the applied state — nothing to remove",
        )
    })?;

    if !force {
        let report = detect_drift(dest, entry).into_diagnostic()?;
        if !report.modified.is_empty() {
            return Err(drift_error(
                &[(target.clone(), report)],
                "tmpl::remove::drift",
            ));
        }
    }

    // Snapshot the entry's file list before mutating state — the
    // `entry` borrow is invalidated by `state.applied.remove`.
    let entry_clone = entry.clone();
    state.applied.remove(&target);

    // Delete files on disk. Missing files are tolerated (deletion is
    // an idempotent operation) but I/O failures are not.
    let mut removed_count: usize = 0;
    for f in &entry_clone.files {
        let abs = dest.join(f.path.as_path().as_str());
        match fs::remove_file(&abs) {
            Ok(()) => removed_count += 1,
            Err(e) if e.kind() == io::ErrorKind::NotFound => {}
            Err(e) => {
                return Err(miette::miette!(
                    code = "tmpl::remove::io",
                    "failed to delete {}: {}",
                    abs.display(),
                    e,
                ));
            }
        }
        prune_empty_parents(dest, &abs);
    }

    // Recompute the Merkle root over the surviving entries.
    let mut hashes: BTreeMap<LayerName, ContentHash> = BTreeMap::new();
    for (name, entry) in &state.applied {
        hashes.insert(name.clone(), entry.content_hash);
    }
    state.merkle_root = merkle_root(&hashes);

    if state.applied.is_empty() {
        // No layers left — remove state.toml itself, leaving an empty
        // `.template/` directory if it was used.
        if state_path.exists() {
            fs::remove_file(&state_path).map_err(|e| {
                miette::miette!(
                    code = "tmpl::remove::io",
                    "failed to clear {}: {}",
                    state_path.display(),
                    e,
                )
            })?;
        }
    } else {
        state.save(&state_path).into_diagnostic()?;
    }

    println!(
        "Removed layer '{target}' ({removed_count} file{plural} deleted, {survivors} layer{splural} remaining).",
        plural = if removed_count == 1 { "" } else { "s" },
        survivors = state.applied.len(),
        splural = if state.applied.len() == 1 { "" } else { "s" },
    );
    Ok(())
}

/// Walk up the parent chain of `start` deleting empty directories
/// until a non-empty directory or `dest` itself is reached. Errors are
/// silently swallowed: best-effort cleanup, not authoritative.
fn prune_empty_parents(dest: &Path, start: &Path) {
    let mut current = start.parent();
    while let Some(dir) = current {
        if dir == dest || !dir.starts_with(dest) {
            return;
        }
        // Read the directory; bail if anything goes wrong.
        let Ok(mut iter) = fs::read_dir(dir) else {
            return;
        };
        if iter.next().is_some() {
            return;
        }
        if fs::remove_dir(dir).is_err() {
            return;
        }
        current = dir.parent();
    }
}

fn new_command(source: &str, dest: &str, public: bool) -> miette::Result<()> {
    let owner_repo = source.strip_prefix("gh:").ok_or_else(|| {
        miette::miette!(
            code = "tmpl::new::source-format",
            help = "Sources must look like `gh:owner/repo` (the GitHub CLI is the only supported transport in Phase C).",
            "unsupported source: {source:?}",
        )
    })?;
    if owner_repo.split('/').count() != 2 {
        return Err(miette::miette!(
            code = "tmpl::new::source-format",
            "expected `gh:owner/repo`, got `gh:{owner_repo}`",
        ));
    }

    if which::which("gh").is_err() {
        return Err(miette::miette!(
            code = "tmpl::new::gh-missing",
            help = "Install the GitHub CLI: https://cli.github.com/",
            "the `gh` CLI is required for `tmpl new` but was not found on PATH",
        ));
    }

    let visibility_flag = if public { "--public" } else { "--private" };

    println!("Creating {dest} from gh:{owner_repo} via gh repo create…");
    let status = ProcessCommand::new("gh")
        .args([
            "repo",
            "create",
            dest,
            "--template",
            owner_repo,
            visibility_flag,
            "--clone",
        ])
        .status()
        .map_err(|e| miette::miette!(code = "tmpl::new::spawn", "failed to spawn gh: {e}",))?;
    if !status.success() {
        return Err(miette::miette!(
            code = "tmpl::new::gh-failed",
            "gh repo create exited with {status}; the GitHub CLI's stderr above has the details",
        ));
    }

    let bootstrap = PathBuf::from(dest).join(".template/bootstrap.sh");
    if !bootstrap.exists() {
        println!(
            "Note: {} was not present in the cloned template — skip bootstrap step.",
            bootstrap.display()
        );
        return Ok(());
    }

    println!("Running bootstrap.sh inside {dest}…");
    let status = ProcessCommand::new("bash")
        .arg(".template/bootstrap.sh")
        .current_dir(dest)
        .status()
        .map_err(|e| {
            miette::miette!(
                code = "tmpl::new::bootstrap-spawn",
                "failed to spawn bash: {e}",
            )
        })?;
    if !status.success() {
        return Err(miette::miette!(
            code = "tmpl::new::bootstrap-failed",
            "bootstrap.sh exited with {status} inside {dest}",
        ));
    }
    println!("Done. The new repository is ready in ./{dest}.");
    Ok(())
}

fn verify(template_root: &Path) -> miette::Result<()> {
    let loaded = Template::<Loaded>::load(template_root).into_diagnostic()?;
    loaded.validate().into_diagnostic()?;
    println!("OK");
    Ok(())
}

fn status(dest: &Path) -> miette::Result<()> {
    let path = dest.join(".template").join("state.toml");
    let state = State::load(&path).into_diagnostic()?;
    if state.applied.is_empty() {
        println!("no layers applied");
    } else {
        println!("merkle_root = {}", state.merkle_root.to_hex());
        for (name, entry) in &state.applied {
            println!(
                "  {name:24}  {hash}  {applied_at}  ({nfiles} file{plural})",
                name = name.as_str(),
                hash = entry.content_hash.to_hex(),
                applied_at = entry.applied_at,
                nfiles = entry.files.len(),
                plural = if entry.files.len() == 1 { "" } else { "s" },
            );
        }
    }
    Ok(())
}

fn seal(template_root: &Path) -> miette::Result<()> {
    if !template_root.exists() {
        println!("`.template/` is already absent — nothing to do");
        return Ok(());
    }
    fs::remove_dir_all(template_root).map_err(|e| {
        miette::miette!(
            code = "tmpl::seal::io",
            "failed to remove {}: {}",
            template_root.display(),
            e
        )
    })?;
    println!(
        "Sealed: {} removed. The repository has graduated from the engine.",
        template_root.display()
    );
    Ok(())
}

/// Iterate every applied layer and collect drift reports for the ones
/// that have any modified or missing files.
fn collect_drift(
    dest: &Path,
    state: &State,
) -> Result<Vec<(LayerName, DriftReport)>, tmpl::TmplError> {
    let mut out = Vec::new();
    for (name, entry) in &state.applied {
        let report = detect_drift(dest, entry)?;
        if !report.is_clean() {
            out.push((name.clone(), report));
        }
    }
    Ok(out)
}

/// Render a `tmpl::add` / `tmpl::remove` drift conflict as a
/// structured `miette` error.
fn drift_error(reports: &[(LayerName, DriftReport)], code: &'static str) -> miette::Report {
    let mut details = String::new();
    for (layer, report) in reports {
        // `Write` is brought into scope as `_` at file top so the
        // `writeln!` calls resolve without polluting the public surface.
        // Writing to a `String` is infallible — surface the
        // theoretical error explicitly so a future API change cannot
        // hide a regression.
        writeln!(details, "layer '{layer}':").expect("write to String never fails");
        for p in &report.modified {
            writeln!(details, "    modified: {}", p.as_path())
                .expect("write to String never fails");
        }
        for p in &report.missing {
            writeln!(details, "    missing:  {}", p.as_path())
                .expect("write to String never fails");
        }
    }
    miette::miette!(
        code = code,
        help = "Inspect the listed files; resolve manually then re-run, or pass `--force` to overwrite local changes.",
        "drift detected against the recorded state:\n{details}",
    )
}

fn parse_layer_names(raw: &[String]) -> miette::Result<Vec<LayerName>> {
    raw.iter()
        .filter(|s| !s.is_empty())
        .map(|s| {
            LayerName::new(s.as_str())
                .map_err(|e| miette::miette!(code = "tmpl::cli", "invalid layer name {s:?}: {e}"))
        })
        .collect()
}

fn build_context(facts: &ProjectFacts<'_>) -> Context {
    let year_str = jiff::Timestamp::now().strftime("%Y").to_string();
    let year: u32 = year_str.parse().unwrap_or(2026);
    Context {
        project: ProjectInfo {
            name: facts.name.to_owned(),
            owner: facts.owner.to_owned(),
            description: facts.description.to_owned(),
            year,
            author: facts.owner.to_owned(),
            repository_url: Some(format!(
                "https://github.com/{owner}/{name}",
                owner = facts.owner,
                name = facts.name,
            )),
        },
        answers: BTreeMap::new(),
    }
}
