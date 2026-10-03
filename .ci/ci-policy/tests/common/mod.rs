use std::{
    fs,
    io::Write,
    path::Path,
    process::{Command, Stdio},
};

mod handoff;

pub struct Repository {
    pub directory: tempfile::TempDir,
}

impl Repository {
    pub fn new() -> Self {
        let value = Self {
            directory: tempfile::tempdir().expect("fixture directory"),
        };
        value.git(&["init", "--template=", "--quiet"]);
        value.git(&["config", "core.autocrlf", "false"]);
        value.git(&["config", "core.fsmonitor", "false"]);
        value
    }

    pub fn path(&self) -> &Path {
        self.directory.path()
    }

    pub fn git(&self, arguments: &[&str]) -> Vec<u8> {
        let output = Command::new("git")
            .current_dir(self.path())
            .args(arguments)
            .output()
            .expect("fixture Git");
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        output.stdout
    }

    pub fn write(&self, path: &str, bytes: &[u8]) {
        let path = self.path().join(path);
        fs::create_dir_all(path.parent().expect("fixture parent")).expect("fixture directory");
        fs::write(&path, bytes).expect("fixture content");
        self.git(&["add", "--", path.to_str().expect("fixture path")]);
    }

    pub fn blob(&self, bytes: &[u8]) -> String {
        let mut child = Command::new("git")
            .current_dir(self.path())
            .args(["hash-object", "-w", "--stdin"])
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .spawn()
            .expect("fixture object writer");
        child
            .stdin
            .take()
            .expect("fixture input")
            .write_all(bytes)
            .expect("fixture object input");
        let output = child.wait_with_output().expect("fixture object output");
        assert!(output.status.success());
        String::from_utf8(output.stdout)
            .expect("object identity")
            .trim()
            .to_owned()
    }

    pub fn entry(&self, mode: &str, object: &str, path: &str) {
        self.git(&[
            "update-index",
            "--add",
            "--cacheinfo",
            &format!("{mode},{object},{path}"),
        ]);
    }

    pub fn commit(&self, parents: &[&str]) -> String {
        let bytes = self.git(&["write-tree"]);
        let tree = std::str::from_utf8(&bytes).expect("fixture tree").trim();
        let mut command = Command::new("git");
        command
            .current_dir(self.path())
            .env("GIT_AUTHOR_NAME", "Fixture")
            .env("GIT_AUTHOR_EMAIL", "fixture@example.com")
            .env("GIT_COMMITTER_NAME", "Fixture")
            .env("GIT_COMMITTER_EMAIL", "fixture@example.com")
            .args([
                "-c",
                "commit.gpgsign=false",
                "commit-tree",
                tree,
                "-m",
                "Fixture",
            ]);
        for parent in parents {
            command.args(["-p", parent]);
        }
        let output = command.output().expect("fixture commit");
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        let revision = String::from_utf8(output.stdout)
            .expect("commit identity")
            .trim()
            .to_owned();
        self.git(&["update-ref", "refs/heads/main", &revision]);
        self.git(&["symbolic-ref", "HEAD", "refs/heads/main"]);
        revision
    }
}
