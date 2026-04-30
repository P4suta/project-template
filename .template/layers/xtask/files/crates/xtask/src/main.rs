//! Dev automation entry point. Add subcommands here as the project's
//! one-shot bash scripts grow into something that wants real argument
//! parsing, structured logging, or testing.
//!
//! Invoke from the workspace root via `cargo xtask <subcommand>`.

#![deny(missing_docs)]

use std::process::ExitCode;

use clap::{Parser, Subcommand};

/// `cargo xtask` CLI surface.
#[derive(Debug, Parser)]
#[command(name = "xtask", version, about = "Project-local dev automation.")]
struct Cli {
    /// Subcommand.
    #[command(subcommand)]
    command: Command,
}

/// Available subcommands.
#[derive(Debug, Subcommand)]
enum Command {
    /// Print "hello, xtask" — placeholder until real automation lands.
    Hello,
}

fn main() -> ExitCode {
    let Cli { command } = Cli::parse();
    match command {
        Command::Hello => {
            println!("hello, xtask");
            ExitCode::SUCCESS
        }
    }
}
