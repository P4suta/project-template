use std::{fs, path::Path, process::Command};

use tmpl::{State, state::applied_paths};

fn render(template: &Path, destination: &Path) -> State {
    let output = Command::new(env!("CARGO_BIN_EXE_tmpl"))
        .arg("--template-root")
        .arg(template)
        .arg("--dest")
        .arg(destination)
        .args([
            "apply",
            "--project-name",
            "smoke-test",
            "--project-owner",
            "P4suta",
            "--project-description",
            "Template verification",
        ])
        .output()
        .expect("run actual template CLI");
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    State::load(&destination.join(".template/state.toml")).expect("rendered state")
}

fn check_placeholders(directory: &Path) {
    for entry in fs::read_dir(directory).expect("rendered directory") {
        let entry = entry.expect("rendered entry");
        if entry.file_name() == ".template" {
            continue;
        }
        let kind = entry.file_type().expect("rendered file kind");
        assert!(
            !kind.is_symlink(),
            "rendered content must not escape the fixture"
        );
        if kind.is_dir() {
            check_placeholders(&entry.path());
        } else {
            let bytes = fs::read(entry.path()).expect("rendered bytes");
            for position in bytes
                .windows(2)
                .enumerate()
                .filter_map(|(index, pair)| (pair == b"__").then_some(index))
            {
                let remainder = &bytes[position + 2..];
                if let Some(end) = remainder.windows(2).position(|pair| pair == b"__") {
                    let name = &remainder[..end];
                    assert!(
                        name.is_empty()
                            || !name
                                .iter()
                                .all(|byte| byte.is_ascii_uppercase() || *byte == b'_'),
                        "unresolved template placeholder in {}",
                        entry.path().display()
                    );
                }
            }
        }
    }
}

#[test]
fn bundled_cli_render_is_nonempty_reproducible_and_resolves_all_placeholders() {
    let template = Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .expect("template directory");
    let first = tempfile::tempdir().expect("first destination");
    let second = tempfile::tempdir().expect("second destination");
    let first_state = render(template, first.path());
    let second_state = render(template, second.path());
    assert!(
        !first_state.applied.is_empty(),
        "the full render must cover bundled layers"
    );
    assert!(
        !applied_paths(&first_state).is_empty(),
        "the full render must produce files"
    );
    assert_eq!(
        first_state.merkle_root, second_state.merkle_root,
        "CLI renders must have identical content identities"
    );
    check_placeholders(first.path());
    check_placeholders(second.path());
}
