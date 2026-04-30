//! `tmpl` binary entry point. Dispatches sub-commands; each sub-command
//! is a thin shell over the library API in [`tmpl::template`].

#![deny(missing_docs)]

use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::ExitCode;

use clap::{Parser, Subcommand};
use miette::IntoDiagnostic;

use tmpl::Context;
use tmpl::ctx::ProjectInfo;
use tmpl::layer::LayerName;
use tmpl::state::State;
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
    /// extended by the new layer; existing files are re-rendered as
    /// well to keep them coherent with the updated capability graph.
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
    },
    /// Remove an applied layer. Reserved for Phase C; the structured
    /// error on invocation points at the workaround.
    Remove {
        /// Layer to remove.
        layer: String,
    },
    /// Run manifest + layer DAG soundness checks. Used by the engine's
    /// own CI as well as `just verify-template`.
    Verify,
    /// Print the current applied state, if any.
    Status,
    /// Delete `.template/` and graduate from the engine. Idempotent —
    /// re-running on a sealed repo is a structured no-op.
    Seal,
    /// Generate a new project from a remote template without using
    /// the GitHub UI. Reserved for Phase C; the structured error on
    /// invocation points at the workaround (`gh repo create
    /// --template` + `.template/bootstrap.sh`).
    New {
        /// Source template, e.g. `gh:P4suta/project-template`.
        source: String,
        /// Destination directory.
        dest: PathBuf,
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

/// Bundled inputs for [`add`]. Same shape as `ApplyInvocation` but with
/// a single layer string.
struct AddInvocation<'a> {
    template_root: &'a Path,
    dest: &'a Path,
    new_layer: &'a str,
    project: ProjectFacts<'a>,
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
        } => add(&AddInvocation {
            template_root: &cli.template_root,
            dest: &cli.dest,
            new_layer: &layer,
            project: ProjectFacts {
                name: &project_name,
                owner: &project_owner,
                description: &project_description,
            },
        }),
        Command::Remove { layer } => remove(&layer),
        Command::Verify => verify(&cli.template_root),
        Command::Status => status(&cli.dest),
        Command::Seal => seal(&cli.template_root),
        Command::New { source, dest } => new_command(&source, &dest),
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

    // Extend the previously-applied selection with the new layer and
    // re-run the full pipeline. The DAG resolver re-orders the
    // combined set; render and apply update both the new layer and
    // any existing layers whose evaluation context depends on it.
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

fn remove(_layer: &str) -> miette::Result<()> {
    Err(miette::miette!(
        code = "tmpl::remove::phase-c",
        help = "Workaround: invoke `tmpl apply --layers <set without this layer> --project-* …` to re-render the destination with a smaller selection. Note that orphaned files are not auto-deleted by that path; clean them up manually for now.",
        "tmpl remove is reserved for Phase C and is not yet implemented",
    ))
}

fn new_command(_source: &str, _dest: &Path) -> miette::Result<()> {
    Err(miette::miette!(
        code = "tmpl::new::phase-c",
        help = "Workaround: `gh repo create --template P4suta/project-template <name>` to create the destination, `gh repo clone <name>` to fetch it, then `bash .template/bootstrap.sh` to run apply.",
        "tmpl new is reserved for Phase C and is not yet implemented",
    ))
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
                "  {name:24}  {hash}  {applied_at}",
                name = name.as_str(),
                hash = entry.content_hash.to_hex(),
                applied_at = entry.applied_at,
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
