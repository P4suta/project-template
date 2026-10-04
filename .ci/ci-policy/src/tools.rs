use std::{collections::BTreeMap, path::Path, process::Command};

use anyhow::{Context, Result, ensure};
use serde::Deserialize;

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Pin {
    tool: String,
    version: String,
}

fn pins() -> Result<BTreeMap<String, Pin>> {
    let pins: BTreeMap<String, Pin> = serde_json::from_str(include_str!("../tools.json"))?;
    ensure!(!pins.is_empty(), "verification tool coverage is empty");
    for pin in pins.values() {
        ensure!(
            pin.version.split('.').count() == 3
                && pin
                    .version
                    .split('.')
                    .all(|part| !part.is_empty() && part.bytes().all(|byte| byte.is_ascii_digit())),
            "verification tool versions must be exact stable identities"
        );
        ensure!(
            !pin.tool.is_empty()
                && pin
                    .tool
                    .bytes()
                    .all(|byte| byte.is_ascii_alphanumeric() || b"-_:/.".contains(&byte)),
            "verification tool identity is invalid"
        );
    }
    Ok(pins)
}

pub fn command(root: &Path, program: &str) -> Result<Command> {
    let pins = pins()?;
    let pin = pins
        .get(program)
        .context("required verification tool has no pin")?;
    let mut command = Command::new("mise");
    command
        .current_dir(root)
        .env("MISE_AUTO_INSTALL", "false")
        .args(["x", &format!("{}@{}", pin.tool, pin.version), "--", program]);
    Ok(command)
}

pub fn install(workflow_only: bool) -> Result<()> {
    let pins = pins()?;
    let directory = tempfile::tempdir()?;
    let mut command = Command::new("mise");
    command.current_dir(directory.path());
    command.arg("install");
    command.args(
        pins.iter()
            .filter(|(program, _)| {
                !workflow_only
                    || matches!(
                        program.as_str(),
                        "actionlint" | "gitleaks" | "shellcheck" | "taplo" | "typos" | "zizmor"
                    )
            })
            .map(|(_, pin)| format!("{}@{}", pin.tool, pin.version)),
    );
    crate::local::execute(&mut command, None)?;
    if workflow_only {
        return Ok(());
    }
    crate::local::execute(self::command(directory.path(), "pwsh")?.args(["-NoProfile", "-NonInteractive", "-Command", "Install-PSResource -Name PSScriptAnalyzer -Version 1.25.0 -Scope CurrentUser -TrustRepository -ErrorAction Stop"]), None)
}

pub fn doctor() -> Result<()> {
    let directory = tempfile::tempdir()?;
    for (program, pin) in pins()? {
        let output = self::command(directory.path(), &program)?
            .arg("--version")
            .output()
            .with_context(|| format!("required {program} could not start"))?;
        let text = format!(
            "{}{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
        ensure!(
            output.status.success() && text.contains(&pin.version),
            "required {program} version {} is unavailable",
            pin.version
        );
    }
    crate::local::execute(
        self::command(directory.path(), "pwsh")?.args([
            "-NoProfile",
            "-NonInteractive",
            "-Command",
            "Import-Module PSScriptAnalyzer -RequiredVersion 1.25.0 -ErrorAction Stop",
        ]),
        None,
    )
}
