pub(super) fn permitted(arguments: &[String]) -> bool {
    let Some(program) = arguments.first().map(String::as_str) else {
        return false;
    };
    let action = arguments.get(1).map(String::as_str);
    if arguments.iter().any(|value| {
        matches!(
            value.as_str(),
            "--write" | "--fix" | "--fix-errors" | "--apply"
        )
    }) {
        return false;
    }
    match program {
        "ci-policy" => matches!(
            action,
            Some("check" | "actionlint" | "prove" | "prove-source")
        ),
        "rustc" | "lean" | "ghc" => true,
        "git" => matches!(
            action,
            Some(
                "cat-file"
                    | "ls-tree"
                    | "rev-parse"
                    | "status"
                    | "diff"
                    | "verify-commit"
                    | "merge-base"
                    | "show"
            )
        ),
        "cargo" => cargo_operation(
            action.unwrap_or("").as_bytes(),
            arguments.iter().any(|value| value == "--check"),
            arguments.get(2).is_some_and(|value| value == "run"),
            arguments.iter().any(|value| value == "check"),
            owned_runner(arguments),
        ),
        "go" => matches!(action, Some("test" | "vet" | "build")),
        "dotnet" => {
            matches!(action, Some("test" | "build"))
                || action == Some("format")
                    && arguments.iter().any(|value| value == "--verify-no-changes")
        }
        "swift" => matches!(action, Some("test" | "build")),
        "bun" => {
            action == Some("test")
                || action == Some("run")
                    && arguments.get(2).is_some_and(|value| {
                        matches!(value.as_str(), "check" | "typecheck" | "lint" | "test")
                    })
        }
        "lake" => action == Some("build"),
        "cabal" => matches!(action, Some("check" | "test" | "build")),
        "typos" => !arguments.iter().any(|value| value == "-w"),
        "actionlint" | "zizmor" | "shellcheck" | "gitleaks" | "taplo" | "tsc" => true,
        _ => false,
    }
}

fn cargo_operation(
    action: &[u8],
    format_check: bool,
    nextest_run: bool,
    dependency_check: bool,
    owned_check: bool,
) -> bool {
    match action {
        b"check" | b"clippy" | b"test" | b"build" | b"llvm-cov" => true,
        b"fmt" => format_check,
        b"nextest" => nextest_run,
        b"deny" => dependency_check,
        b"run" => owned_check,
        _ => false,
    }
}

fn runner_operation(action: &[u8]) -> bool {
    matches!(
        action,
        b"check"
            | b"check-all"
            | b"check-rust"
            | b"verify"
            | b"ci"
            | b"test"
            | b"lint"
            | b"fmt-check"
            | b"prove"
            | b"prove-source"
            | b"audit"
    )
}

fn owned_runner(arguments: &[String]) -> bool {
    let Some(separator) = arguments.iter().position(|value| value == "--") else {
        return false;
    };
    let Some(action) = arguments.get(separator + 1) else {
        return false;
    };
    let owned = arguments[..separator].windows(2).any(|pair| {
        matches!(pair[0].as_str(), "--package" | "-p" | "--bin")
            && matches!(pair[1].as_str(), "xtask" | "ci-policy" | "tmpl")
            || pair[0] == "--manifest-path"
                && (matches!(
                    pair[1].as_str(),
                    ".ci/ci-policy/Cargo.toml" | ".template/tmpl/Cargo.toml" | "xtask/Cargo.toml"
                ) || pair[1].ends_with("/xtask/Cargo.toml"))
    });
    owned && runner_operation(action.as_bytes())
}

#[cfg(kani)]
mod proofs {
    use super::{cargo_operation, runner_operation};

    #[kani::proof]
    #[kani::unwind(17)]
    fn cargo_operations_do_not_admit_publication() {
        let bytes: [u8; 16] = kani::any();
        let length: u8 = kani::any();
        kani::assume(length <= 16);
        let action = &bytes[..usize::from(length)];
        let format_check = kani::any();
        let nextest_run = kani::any();
        let dependency_check = kani::any();
        let owned_check = kani::any();
        let direct = cargo_operation(
            action,
            format_check,
            nextest_run,
            dependency_check,
            owned_check,
        );
        assert_eq!(
            direct,
            matches!(
                action,
                b"check" | b"clippy" | b"test" | b"build" | b"llvm-cov"
            ) || action == b"fmt" && format_check
                || action == b"nextest" && nextest_run
                || action == b"deny" && dependency_check
                || action == b"run" && owned_check
        );
        let owned = runner_operation(action);
        assert_eq!(
            owned,
            matches!(
                action,
                b"check"
                    | b"check-all"
                    | b"check-rust"
                    | b"verify"
                    | b"ci"
                    | b"test"
                    | b"lint"
                    | b"fmt-check"
                    | b"prove"
                    | b"prove-source"
                    | b"audit"
            )
        );
        if matches!(action, b"publish" | b"release" | b"sign") {
            assert!(!direct && !owned);
        }
        kani::cover!(direct);
        kani::cover!(owned);
        kani::cover!(!direct && !owned);
        kani::cover!(action == b"publish");
    }
}

#[cfg(test)]
mod tests {
    use super::permitted;

    #[test]
    fn publishing_and_interpreter_commands_are_not_verification() {
        for command in [
            vec!["cargo", "publish"],
            vec!["dotnet", "publish"],
            vec!["gh", "release", "create"],
            vec!["rustup", "update"],
            vec!["sh", "-c", "true"],
            vec!["cargo", "run", "--package", "xtask", "--", "release"],
            vec!["cargo", "fmt"],
            vec!["typos", "--write"],
        ] {
            assert!(!permitted(
                &command.into_iter().map(str::to_owned).collect::<Vec<_>>()
            ));
        }
    }
}
