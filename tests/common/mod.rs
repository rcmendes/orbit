#![allow(dead_code)]

use std::ffi::OsString;
use std::path::{Path, PathBuf};
use std::process::{Command, Output, Stdio};

use tempfile::TempDir;

pub struct TestRepo {
    dir: TempDir,
    global_config: PathBuf,
    home: PathBuf,
    bin: PathBuf,
    path: OsString,
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

        let home = dir.path().join("home");
        let bin = home.join("bin");
        let tmp = home.join("tmp");
        std::fs::create_dir_all(&bin).unwrap();
        std::fs::create_dir_all(&tmp).unwrap();
        let path = isolated_path(&bin);

        let repo = Self {
            dir,
            global_config,
            home,
            bin,
            path,
        };
        repo.git(&["init", "-q"]);
        repo.git(&["config", "user.name", "Test"]);
        repo.git(&["config", "user.email", "test@example.com"]);
        repo.git(&["commit", "--allow-empty", "-q", "-m", "init"]);
        repo
    }

    pub fn home(&self) -> &Path {
        &self.home
    }

    pub fn bin(&self) -> &Path {
        &self.bin
    }

    /// Create an executable in the isolated PATH so detection can find it.
    pub fn fake_command(&self, name: &str) {
        let path = self.bin.join(name);
        std::fs::write(&path, "#!/bin/sh\nexit 0\n").unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mut permissions = std::fs::metadata(&path).unwrap().permissions();
            permissions.set_mode(0o755);
            std::fs::set_permissions(&path, permissions).unwrap();
        }
    }

    pub fn path(&self) -> &Path {
        self.dir.path()
    }

    pub fn board_path(&self) -> PathBuf {
        self.path().join(".orbit")
    }

    pub fn legacy_board_path(&self) -> PathBuf {
        self.path().join(".git/orbit/board")
    }

    pub fn set_lock_timeout(&self, millis: u64) {
        let path = self.board_path().join("config.toml");
        let config = std::fs::read_to_string(&path).unwrap();
        let config = config
            .lines()
            .map(|line| {
                if line.trim_start().starts_with("lock_timeout_ms") {
                    format!("lock_timeout_ms = {millis}")
                } else {
                    line.to_string()
                }
            })
            .collect::<Vec<_>>()
            .join("\n");
        std::fs::write(path, format!("{config}\n")).unwrap();
    }

    pub fn init(&self) {
        self.ok_json(&["init", "--agents", "none", "--slug", "orb", "--json"]);
    }

    pub fn init_at(&self, dir: &Path) -> serde_json::Value {
        self.ok_json(&["init", dir.to_str().unwrap(), "--agents", "none", "--json"])
    }

    pub fn link_at(&self, dir: &Path) -> serde_json::Value {
        self.ok_json(&["link", dir.to_str().unwrap(), "--json"])
    }

    /// Creates a Git repository in a named subdirectory, so auto-adoption can
    /// derive a stable slug from the folder name.
    pub fn named_repo(&self, name: &str) -> PathBuf {
        let path = self.path().join(name);
        std::fs::create_dir_all(&path).unwrap();
        for args in [
            &["init", "-q"][..],
            &["config", "user.name", "Test"][..],
            &["config", "user.email", "test@example.com"][..],
            &["commit", "--allow-empty", "-q", "-m", "init"][..],
        ] {
            let output = self.command("git", &path, args).output().unwrap();
            assert!(
                output.status.success(),
                "git {args:?} failed: {}",
                String::from_utf8_lossy(&output.stderr)
            );
        }
        path
    }

    pub fn git_in(&self, cwd: &Path, args: &[&str]) -> Output {
        self.command("git", cwd, args).output().unwrap()
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
        self.json_in(self.path(), args)
    }

    pub fn json_in(&self, cwd: &Path, args: &[&str]) -> (i32, serde_json::Value) {
        self.parse_json(args, self.orbit_in(cwd, args))
    }

    fn parse_json(&self, args: &[&str], output: Output) -> (i32, serde_json::Value) {
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

    pub fn json_with_stdin(&self, args: &[&str], input: &str) -> (i32, serde_json::Value) {
        let input_path = self.home.join("stdin.txt");
        std::fs::write(&input_path, input).unwrap();
        let mut command = self.orbit_command(args);
        command.stdin(Stdio::from(
            std::fs::File::open(&input_path).expect("open stdin fixture"),
        ));
        self.parse_json(args, command.output().unwrap())
    }

    pub fn json_with_env(&self, args: &[&str], envs: &[(&str, &str)]) -> (i32, serde_json::Value) {
        self.parse_json(args, self.orbit_with_env(args, envs))
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

    pub fn ok_json_with_env(&self, args: &[&str], envs: &[(&str, &str)]) -> serde_json::Value {
        let (status, value) = self.json_with_env(args, envs);
        assert_eq!(
            status,
            0,
            "command {args:?} failed: {}",
            serde_json::to_string_pretty(&value).unwrap()
        );
        assert_eq!(value["ok"], true, "command {args:?}: {value}");
        value
    }

    pub fn ok_json_in(&self, cwd: &Path, args: &[&str]) -> serde_json::Value {
        let (status, value) = self.json_in(cwd, args);
        assert_eq!(
            status,
            0,
            "command {args:?} failed: {}",
            serde_json::to_string_pretty(&value).unwrap()
        );
        assert_eq!(value["ok"], true, "command {args:?}: {value}");
        value
    }

    pub fn json_in_with_env(
        &self,
        cwd: &Path,
        args: &[&str],
        envs: &[(&str, &str)],
    ) -> (i32, serde_json::Value) {
        let mut command = self.command(env!("CARGO_BIN_EXE_orbit"), cwd, args);
        for (key, value) in envs {
            command.env(key, value);
        }
        self.parse_json(args, command.output().unwrap())
    }

    pub fn ok_json_in_with_env(
        &self,
        cwd: &Path,
        args: &[&str],
        envs: &[(&str, &str)],
    ) -> serde_json::Value {
        let (status, value) = self.json_in_with_env(cwd, args, envs);
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

    /// Rewrite the lease to a time in the past so the claim is stale.
    pub fn make_stale(&self, id: &str) {
        let content = self.read_task(id);
        let updated = content
            .lines()
            .map(|line| {
                if line.starts_with("lease_until:") {
                    "lease_until: 2020-01-01T00:00:00Z".to_string()
                } else {
                    line.to_string()
                }
            })
            .collect::<Vec<_>>()
            .join("\n");
        self.write_task(id, &(updated + "\n"));
    }

    fn command(&self, program: &str, cwd: &Path, args: &[&str]) -> Command {
        let mut command = Command::new(program);
        command.current_dir(cwd);
        command.args(args);
        command.env("GIT_CONFIG_GLOBAL", &self.global_config);
        command.env("HOME", &self.home);
        command.env("USERPROFILE", &self.home);
        command.env("XDG_CONFIG_HOME", self.home.join(".config"));
        command.env("TMPDIR", self.home.join("tmp"));
        command.env("TMP", self.home.join("tmp"));
        command.env("TEMP", self.home.join("tmp"));
        command.env("PATH", &self.path);
        command.env_remove("ORBIT_DIR");
        command.env_remove("ORBIT_AGENT");
        command
    }
}

fn isolated_path(bin: &Path) -> OsString {
    let mut dirs = vec![bin.to_path_buf()];
    if let Some(git) = find_git() {
        let linked = link_git(bin, &git);
        if !linked && let Some(parent) = git.parent() {
            dirs.push(parent.to_path_buf());
        }
    }
    #[cfg(windows)]
    if let Some(root) = std::env::var_os("SystemRoot") {
        dirs.push(PathBuf::from(root).join("System32"));
    }
    std::env::join_paths(dirs).expect("build an isolated PATH")
}

#[cfg(unix)]
fn link_git(bin: &Path, git: &Path) -> bool {
    let link = bin.join("git");
    if std::fs::symlink_metadata(&link).is_ok() {
        return true;
    }
    std::os::unix::fs::symlink(git, &link).is_ok()
}

#[cfg(not(unix))]
fn link_git(_bin: &Path, _git: &Path) -> bool {
    false
}

fn find_git() -> Option<PathBuf> {
    let path = std::env::var_os("PATH")?;
    let names: &[&str] = if cfg!(windows) {
        &["git.exe"]
    } else {
        &["git"]
    };
    for dir in std::env::split_paths(&path) {
        for name in names {
            let candidate = dir.join(name);
            if candidate.is_file() {
                return Some(candidate);
            }
        }
    }
    None
}
