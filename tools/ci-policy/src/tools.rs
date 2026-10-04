use std::{collections::BTreeMap, path::Path, process::Command};

use anyhow::{Context, Result, ensure};
use serde::Deserialize;

#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Deserialize)]
#[serde(rename_all = "lowercase")]
#[cfg_attr(kani, derive(kani::Arbitrary))]
enum Tool {
    Actionlint,
    Gitleaks,
    Pwsh,
    Shellcheck,
    Taplo,
    Typos,
    Zizmor,
}

const TOOLS: [Tool; 7] = [
    Tool::Actionlint,
    Tool::Gitleaks,
    Tool::Pwsh,
    Tool::Shellcheck,
    Tool::Taplo,
    Tool::Typos,
    Tool::Zizmor,
];

impl Tool {
    fn program(self) -> &'static str {
        match self {
            Self::Actionlint => "actionlint",
            Self::Gitleaks => "gitleaks",
            Self::Pwsh => "pwsh",
            Self::Shellcheck => "shellcheck",
            Self::Taplo => "taplo",
            Self::Typos => "typos",
            Self::Zizmor => "zizmor",
        }
    }

    fn dependencies(self) -> &'static [Self] {
        match self {
            Self::Actionlint => &[Self::Actionlint, Self::Shellcheck],
            Self::Gitleaks => &[Self::Gitleaks],
            Self::Pwsh => &[Self::Pwsh],
            Self::Shellcheck => &[Self::Shellcheck],
            Self::Taplo => &[Self::Taplo],
            Self::Typos => &[Self::Typos],
            Self::Zizmor => &[Self::Zizmor],
        }
    }
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Pin {
    tool: String,
    version: String,
}

fn pins() -> Result<BTreeMap<Tool, Pin>> {
    let pins: BTreeMap<Tool, Pin> = serde_json::from_str(include_str!("../tools.json"))?;
    ensure!(
        pins.len() == TOOLS.len() && TOOLS.iter().all(|tool| pins.contains_key(tool)),
        "verification tool coverage is incomplete"
    );
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
    let tool = TOOLS
        .into_iter()
        .find(|tool| tool.program() == program)
        .context("required verification tool has no pin")?;
    let mut command = Command::new("mise");
    command
        .current_dir(root)
        .env("MISE_AUTO_INSTALL", "false")
        .arg("x");
    for dependency in tool.dependencies() {
        let pin = pins.get(dependency).context("tool dependency has no pin")?;
        command.arg(format!("{}@{}", pin.tool, pin.version));
    }
    command.args(["--", program]);
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
                        program,
                        Tool::Actionlint
                            | Tool::Gitleaks
                            | Tool::Shellcheck
                            | Tool::Taplo
                            | Tool::Typos
                            | Tool::Zizmor
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
    for (tool, pin) in pins()? {
        let program = tool.program();
        let output = self::command(directory.path(), program)?
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

#[cfg(kani)]
mod proofs {
    use super::Tool;

    #[kani::proof]
    #[kani::unwind(3)]
    fn every_tool_activates_its_complete_dependency_set() {
        let tool: Tool = kani::any();
        let dependencies = tool.dependencies();
        assert!(dependencies.contains(&tool));
        assert_eq!(
            dependencies.contains(&Tool::Shellcheck),
            matches!(tool, Tool::Actionlint | Tool::Shellcheck)
        );
        assert_eq!(
            dependencies.len(),
            if tool == Tool::Actionlint { 2 } else { 1 }
        );
        kani::cover!(tool == Tool::Actionlint);
        kani::cover!(tool == Tool::Pwsh);
    }
}
