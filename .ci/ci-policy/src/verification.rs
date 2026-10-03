#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[cfg_attr(kani, derive(kani::Arbitrary))]
pub enum FileKind {
    Other,
    Shell,
    Powershell,
    Toml,
    Json,
    Yaml,
    Workflow,
    Action,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[cfg_attr(kani, derive(kani::Arbitrary))]
pub enum Check {
    Skills,
    Spelling,
    Secrets,
    Shell,
    Powershell,
    Toml,
    Json,
    Yaml,
    Workflow,
    Actionlint,
    Zizmor,
}

#[derive(Clone, Copy, PartialEq, Eq)]
#[cfg_attr(kani, derive(kani::Arbitrary))]
pub enum Context {
    Personal,
    Repository,
}

const ALL_CHECKS: [Check; 11] = [
    Check::Skills,
    Check::Spelling,
    Check::Secrets,
    Check::Shell,
    Check::Powershell,
    Check::Toml,
    Check::Json,
    Check::Yaml,
    Check::Workflow,
    Check::Actionlint,
    Check::Zizmor,
];

#[derive(Clone, Copy)]
struct Plan {
    selected: [bool; 11],
}

impl Plan {
    fn new(context: Context) -> Self {
        let mut selected = [false; 11];
        selected[Check::Skills as usize] = context == Context::Personal;
        selected[Check::Secrets as usize] = true;
        Self { selected }
    }

    fn include(&mut self, kind: FileKind) {
        self.selected[Check::Spelling as usize] = true;
        self.selected[Check::Secrets as usize] = true;
        match kind {
            FileKind::Other => {}
            FileKind::Shell => self.selected[Check::Shell as usize] = true,
            FileKind::Powershell => self.selected[Check::Powershell as usize] = true,
            FileKind::Toml => self.selected[Check::Toml as usize] = true,
            FileKind::Json => self.selected[Check::Json as usize] = true,
            FileKind::Yaml => self.selected[Check::Yaml as usize] = true,
            FileKind::Workflow => {
                self.selected[Check::Yaml as usize] = true;
                self.selected[Check::Workflow as usize] = true;
                self.selected[Check::Actionlint as usize] = true;
                self.selected[Check::Zizmor as usize] = true;
            }
            FileKind::Action => {
                self.selected[Check::Yaml as usize] = true;
                self.selected[Check::Zizmor as usize] = true;
            }
        }
    }
}

pub fn checks(kinds: &[FileKind]) -> Vec<Check> {
    checks_for(Context::Personal, kinds)
}

pub fn checks_for(context: Context, kinds: &[FileKind]) -> Vec<Check> {
    let mut plan = Plan::new(context);
    for kind in kinds {
        plan.include(*kind);
    }
    ALL_CHECKS
        .into_iter()
        .filter(|check| plan.selected[*check as usize])
        .collect()
}

pub fn file_kind(path: &str, bytes: &[u8]) -> FileKind {
    let extension = path.rsplit_once('.').map(|(_, extension)| extension);
    if path.starts_with(".github/workflows/") && matches!(extension, Some("yml" | "yaml")) {
        return FileKind::Workflow;
    }
    if matches!(path.rsplit('/').next(), Some("action.yml" | "action.yaml")) {
        return FileKind::Action;
    }
    match extension {
        Some("sh" | "bash") => FileKind::Shell,
        Some("ps1" | "psm1" | "psd1") => FileKind::Powershell,
        Some("toml") => FileKind::Toml,
        Some("json") => FileKind::Json,
        Some("yml" | "yaml") => FileKind::Yaml,
        _ => {
            let first_line = bytes
                .split(|byte| *byte == b'\n')
                .next()
                .unwrap_or_default();
            if [
                b"#!/bin/sh".as_slice(),
                b"#!/bin/bash",
                b"#!/usr/bin/env sh",
                b"#!/usr/bin/env bash",
            ]
            .contains(&first_line)
            {
                FileKind::Shell
            } else {
                FileKind::Other
            }
        }
    }
}

#[cfg(kani)]
mod proofs {
    use super::{Check, Context, FileKind, Plan};

    #[kani::proof]
    #[kani::unwind(12)]
    fn adding_a_file_cannot_remove_a_required_check() {
        let mut plan = Plan {
            selected: kani::any(),
        };
        let before = plan.selected;
        let kind: FileKind = kani::any();
        plan.include(kind);
        for index in 0..before.len() {
            assert!(!before[index] || plan.selected[index]);
        }
        assert!(plan.selected[Check::Spelling as usize]);
        assert!(plan.selected[Check::Secrets as usize]);
        if kind == FileKind::Workflow {
            assert!(plan.selected[Check::Yaml as usize]);
            assert!(plan.selected[Check::Workflow as usize]);
            assert!(plan.selected[Check::Actionlint as usize]);
            assert!(plan.selected[Check::Zizmor as usize]);
        }
        assert!(Plan::new(Context::Personal).selected[Check::Skills as usize]);
        assert!(Plan::new(Context::Repository).selected[Check::Secrets as usize]);
        assert!(Plan::new(Context::Personal).selected[Check::Secrets as usize]);
        kani::cover!(kind == FileKind::Workflow);
        kani::cover!(kind == FileKind::Other);
    }

    #[kani::proof]
    #[kani::unwind(12)]
    fn every_check_has_a_unique_plan_position() {
        let check: Check = kani::any();
        assert_eq!(super::ALL_CHECKS[check as usize], check);
        assert!(Plan::new(Context::Personal).selected.len() == super::ALL_CHECKS.len());
        kani::cover!(check == Check::Skills);
        kani::cover!(check == Check::Secrets);
    }
}
