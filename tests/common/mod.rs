#![allow(dead_code)]

use std::path::{Path, PathBuf};
use std::process::{Command, Output};

use tempfile::TempDir;

pub struct TestRepo {
    dir: TempDir,
    global_config: PathBuf,
}

impl TestRepo {
    pub fn new() -> Self {
        let dir = tempfile::tempdir().unwrap();
        let global_config = dir.path().join("gitconfig");
        std::fs::write(
            &global_config,
            "[user]\n\tname = Test\n\temail = test@example.com\n",
        )
        .unwrap();

        let repo = Self { dir, global_config };
        repo.git(&["init", "-q"]);
        repo.git(&["config", "user.name", "Test"]);
        repo.git(&["config", "user.email", "test@example.com"]);
        repo.git(&["commit", "--allow-empty", "-q", "-m", "init"]);
        repo
    }

    pub fn path(&self) -> &Path {
        self.dir.path()
    }

    pub fn board_path(&self) -> PathBuf {
        self.path().join(".git/orbit/board")
    }

    pub fn init(&self) {
        self.ok_json(&["init", "--json"]);
    }

    pub fn git(&self, args: &[&str]) -> Output {
        self.command("git", self.path(), args).output().unwrap()
    }

    pub fn orbit(&self, args: &[&str]) -> Output {
        self.orbit_command(args).output().unwrap()
    }

    pub fn orbit_in(&self, cwd: &Path, args: &[&str]) -> Output {
        self.command(env!("CARGO_BIN_EXE_orbit"), cwd, args)
            .output()
            .unwrap()
    }

    pub fn orbit_command(&self, args: &[&str]) -> Command {
        self.command(env!("CARGO_BIN_EXE_orbit"), self.path(), args)
    }

    pub fn orbit_with_env(&self, args: &[&str], envs: &[(&str, &str)]) -> Output {
        let mut command = self.orbit_command(args);
        for (key, value) in envs {
            command.env(key, value);
        }
        command.output().unwrap()
    }

    pub fn json(&self, args: &[&str]) -> (i32, serde_json::Value) {
        let output = self.orbit(args);
        let status = output.status.code().unwrap_or(-1);
        let value = serde_json::from_slice(&output.stdout).unwrap_or_else(|error| {
            panic!(
                "invalid JSON from orbit {args:?}: {error}\nstdout: {}\nstderr: {}",
                String::from_utf8_lossy(&output.stdout),
                String::from_utf8_lossy(&output.stderr)
            )
        });
        (status, value)
    }

    pub fn ok_json(&self, args: &[&str]) -> serde_json::Value {
        let (status, value) = self.json(args);
        assert_eq!(
            status,
            0,
            "command {args:?} failed: {}",
            serde_json::to_string_pretty(&value).unwrap()
        );
        assert_eq!(value["ok"], true, "command {args:?}: {value}");
        value
    }

    pub fn err_json(&self, args: &[&str]) -> (i32, serde_json::Value) {
        let (status, value) = self.json(args);
        assert_ne!(
            status, 0,
            "command {args:?} unexpectedly succeeded: {value}"
        );
        assert_eq!(value["ok"], false, "command {args:?}: {value}");
        (status, value)
    }

    pub fn add(&self, title: &str, extra: &[&str]) -> String {
        let mut args = vec!["add", title, "--json"];
        args.extend_from_slice(extra);
        let value = self.ok_json(&args);
        value["data"]["id"]
            .as_str()
            .unwrap_or_else(|| panic!("add returned no id: {value}"))
            .to_string()
    }

    pub fn add_worktree(&self, path: &Path, branch: &str) {
        let output = self.git(&[
            "worktree",
            "add",
            "-q",
            path.to_str().unwrap(),
            "-b",
            branch,
        ]);
        assert!(
            output.status.success(),
            "worktree add failed: {}",
            String::from_utf8_lossy(&output.stderr)
        );
    }

    pub fn read_task(&self, id: &str) -> String {
        std::fs::read_to_string(self.task_path(id)).unwrap()
    }

    pub fn read(&self, relative: &str) -> String {
        std::fs::read_to_string(self.path().join(relative)).unwrap()
    }

    pub fn read_json(&self, relative: &str) -> serde_json::Value {
        serde_json::from_str(&self.read(relative)).unwrap()
    }

    pub fn exists(&self, relative: &str) -> bool {
        self.path().join(relative).exists()
    }

    pub fn write_task(&self, id: &str, content: &str) {
        std::fs::write(self.task_path(id), content).unwrap();
    }

    pub fn task_path(&self, id: &str) -> PathBuf {
        self.board_path().join("tasks").join(format!("{id}.md"))
    }

    fn command(&self, program: &str, cwd: &Path, args: &[&str]) -> Command {
        let mut command = Command::new(program);
        command.current_dir(cwd);
        command.args(args);
        command.env("GIT_CONFIG_GLOBAL", &self.global_config);
        command.env_remove("ORBIT_DIR");
        command.env_remove("ORBIT_AGENT");
        command
    }
}
