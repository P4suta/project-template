use std::fs;

use super::Repository;
use ci_policy::handoff::patch_files;

#[test]
fn initialization_patch_preserves_binary_content_and_template_deletions() {
    let repository = Repository::new();
    repository.write(".template/engine", b"Old engine\n");
    repository.write("README.md", b"Template\n");
    let original = repository.commit(&[]);
    let rendered = tempfile::tempdir().expect("rendered directory");
    fs::write(rendered.path().join("README.md"), b"Initialized\n").expect("rendered readme");
    fs::write(rendered.path().join("asset.bin"), [0, 255, 128, 0, 13, 10]).expect("binary asset");
    let output = tempfile::tempdir().expect("artifact directory");
    let patch = output.path().join("initialized.patch");
    let names = ["README.md".to_owned(), "asset.bin".to_owned()];
    patch_files(repository.path(), rendered.path(), &names, &patch).expect("complete patch");
    assert_eq!(
        std::str::from_utf8(&repository.git(&["rev-parse", "HEAD"]))
            .expect("original revision")
            .trim(),
        original
    );
    assert_eq!(
        fs::read(repository.path().join(".template/engine")).expect("preserved source"),
        b"Old engine\n"
    );
    assert!(
        repository
            .git_command()
            .args(["apply", "--index"])
            .arg(&patch)
            .status()
            .expect("apply patch")
            .success()
    );
    assert!(!repository.path().join(".template/engine").exists());
    assert_eq!(
        fs::read(repository.path().join("asset.bin")).expect("binary asset"),
        [0, 255, 128, 0, 13, 10]
    );
    assert_eq!(
        repository.git(&["ls-files", "-z"]),
        b"README.md\0asset.bin\0"
    );
    assert!(patch_files(repository.path(), rendered.path(), &names, &patch).is_err());
}

#[test]
fn invalid_inventory_cannot_publish_an_initialization_artifact() {
    let repository = Repository::new();
    repository.write("README.md", b"Template\n");
    repository.commit(&[]);
    let rendered = tempfile::tempdir().expect("rendered directory");
    fs::write(rendered.path().join("README.md"), b"Initialized\n").expect("rendered file");
    let output = tempfile::tempdir().expect("artifact directory");
    let patch = output.path().join("initialized.patch");
    for names in [
        vec![],
        vec!["../README.md".to_owned()],
        vec![".git/config".to_owned()],
        vec!["README.md".to_owned(), "README.md".to_owned()],
        vec!["README.md\0other".to_owned()],
    ] {
        assert!(patch_files(repository.path(), rendered.path(), &names, &patch).is_err());
        assert!(!patch.exists());
    }
}
