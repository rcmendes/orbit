use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{Command, Output, Stdio};

use crate::error::{OrbitError, Result};

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CommitOutcome {
    Committed,
    NoChanges,
    Skipped,
    Failed(String),
}

pub fn common_dir() -> Result<PathBuf> {
    let output = run_git(
        None,
        &["rev-parse", "--path-format=absolute", "--git-common-dir"],
        None,
    )?;
    let value = String::from_utf8_lossy(&output.stdout).trim().to_string();
    if value.is_empty() {
        return Err(OrbitError::git("git did not report a common directory"));
    }

    let path = PathBuf::from(value);
    let absolute = if path.is_absolute() {
        path
    } else {
        std::env::current_dir()
            .map_err(|error| OrbitError::io("resolve current directory", error))?
            .join(path)
    };
    dunce::canonicalize(&absolute)
        .map_err(|error| OrbitError::io(format!("resolve {}", absolute.display()), error))
}

pub fn repo_root() -> Result<PathBuf> {
    let output = run_git(None, &["rev-parse", "--show-toplevel"], None)?;
    let value = String::from_utf8_lossy(&output.stdout).trim().to_string();
    if value.is_empty() {
        return Err(OrbitError::git("git did not report a repository root"));
    }
    Ok(PathBuf::from(value))
}

pub fn config_get(cwd: &Path, key: &str) -> Result<Option<String>> {
    let output = run_git_raw(Some(cwd), &["config", "--get", key], None)?;
    if output.status.success() {
        let value = String::from_utf8_lossy(&output.stdout).trim().to_string();
        if value.is_empty() {
            Ok(None)
        } else {
            Ok(Some(value))
        }
    } else if output.status.code() == Some(1) {
        Ok(None)
    } else {
        Err(git_failure(&["config", "--get", key], &output))
    }
}

pub fn ensure_identity(cwd: &Path) -> Result<()> {
    let name = config_get(cwd, "user.name")?;
    let email = config_get(cwd, "user.email")?;
    if name.is_none() || email.is_none() {
        return Err(OrbitError::config(
            "git user.name and user.email must be configured before `orbit init`",
        ));
    }
    Ok(())
}

pub fn branch_exists(repo: &Path, branch: &str) -> Result<bool> {
    let reference = format!("refs/heads/{branch}");
    let output = run_git_raw(
        Some(repo),
        &["show-ref", "--verify", "--quiet", &reference],
        None,
    )?;
    match output.status.code() {
        Some(0) => Ok(true),
        Some(1) => Ok(false),
        _ => Err(git_failure(
            &["show-ref", "--verify", "--quiet", &reference],
            &output,
        )),
    }
}

pub fn branch_contains_file(repo: &Path, branch: &str, file: &str) -> Result<bool> {
    let object = format!("{branch}:{file}");
    let output = run_git_raw(Some(repo), &["cat-file", "-e", &object], None)?;
    match output.status.code() {
        Some(0) => Ok(true),
        Some(1) | Some(128) => Ok(false),
        _ => Err(git_failure(&["cat-file", "-e", &object], &output)),
    }
}

pub fn empty_tree(repo: &Path) -> Result<String> {
    let output = run_git(Some(repo), &["mktree"], Some(""))?;
    let tree = String::from_utf8_lossy(&output.stdout).trim().to_string();
    if tree.is_empty() {
        return Err(OrbitError::git("git mktree returned an empty tree hash"));
    }
    Ok(tree)
}

pub fn commit_tree(repo: &Path, tree: &str, message: &str) -> Result<String> {
    let output = run_git(
        Some(repo),
        &[
            "-c",
            "commit.gpgsign=false",
            "commit-tree",
            tree,
            "-m",
            message,
        ],
        None,
    )?;
    let commit = String::from_utf8_lossy(&output.stdout).trim().to_string();
    if commit.is_empty() {
        return Err(OrbitError::git("git commit-tree returned no commit hash"));
    }
    Ok(commit)
}

pub fn create_branch(repo: &Path, branch: &str, commit: &str) -> Result<()> {
    run_git(Some(repo), &["branch", branch, commit], None)?;
    Ok(())
}

pub fn delete_branch(repo: &Path, branch: &str) -> Result<()> {
    run_git(Some(repo), &["branch", "-D", branch], None)?;
    Ok(())
}

pub fn worktree_add(repo: &Path, path: &Path, branch: &str) -> Result<()> {
    run_git(
        Some(repo),
        &["worktree", "add", path.to_string_lossy().as_ref(), branch],
        None,
    )?;
    Ok(())
}

pub fn worktree_remove(repo: &Path, path: &Path) -> Result<()> {
    run_git(
        Some(repo),
        &[
            "worktree",
            "remove",
            "--force",
            path.to_string_lossy().as_ref(),
        ],
        None,
    )?;
    Ok(())
}

pub fn commit(cwd: &Path, message: &str) -> Result<CommitOutcome> {
    if status_porcelain(cwd)?.trim().is_empty() {
        return Ok(CommitOutcome::NoChanges);
    }

    run_git(Some(cwd), &["add", "-A"], None)?;
    let output = run_git_raw(
        Some(cwd),
        &[
            "-c",
            "commit.gpgsign=false",
            "commit",
            "--no-gpg-sign",
            "-m",
            message,
        ],
        None,
    )?;

    if output.status.success() {
        return Ok(CommitOutcome::Committed);
    }

    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    let combined = format!("{stdout}\n{stderr}");
    if combined.contains("nothing to commit")
        || combined.contains("nothing added to commit")
        || combined.contains("no changes added to commit")
    {
        return Ok(CommitOutcome::NoChanges);
    }

    Err(git_failure(&["commit", "-m", message], &output))
}

pub fn status_porcelain(cwd: &Path) -> Result<String> {
    let output = run_git(Some(cwd), &["status", "--porcelain"], None)?;
    Ok(String::from_utf8_lossy(&output.stdout).to_string())
}

pub fn current_branch(cwd: &Path) -> Result<String> {
    let output = run_git(Some(cwd), &["rev-parse", "--abbrev-ref", "HEAD"], None)?;
    Ok(String::from_utf8_lossy(&output.stdout).trim().to_string())
}

pub fn is_git_repo(path: &Path) -> bool {
    run_git_raw(Some(path), &["rev-parse", "--git-dir"], None)
        .map(|output| output.status.success())
        .unwrap_or(false)
}

pub fn has_head(path: &Path) -> Result<bool> {
    let output = run_git_raw(Some(path), &["rev-parse", "--verify", "HEAD"], None)?;
    Ok(output.status.success())
}

pub fn set_head(path: &Path, branch: &str) -> Result<()> {
    let reference = format!("refs/heads/{branch}");
    run_git(Some(path), &["symbolic-ref", "HEAD", &reference], None)?;
    Ok(())
}

pub fn init_repo(path: &Path) -> Result<()> {
    run_git(None, &["init", path.to_string_lossy().as_ref()], None)?;
    Ok(())
}

fn run_git(cwd: Option<&Path>, args: &[&str], stdin: Option<&str>) -> Result<Output> {
    let output = run_git_raw(cwd, args, stdin)?;
    if output.status.success() {
        Ok(output)
    } else {
        Err(git_failure(args, &output))
    }
}

fn run_git_raw(cwd: Option<&Path>, args: &[&str], stdin: Option<&str>) -> Result<Output> {
    let mut command = Command::new("git");
    if let Some(cwd) = cwd {
        command.arg("-C").arg(cwd);
    }
    command.args(args);
    command.stdout(Stdio::piped()).stderr(Stdio::piped());
    if stdin.is_some() {
        command.stdin(Stdio::piped());
    } else {
        command.stdin(Stdio::null());
    }

    let mut child = command
        .spawn()
        .map_err(|error| OrbitError::git(format!("could not run git: {error}")))?;

    if let Some(input) = stdin
        && let Some(mut pipe) = child.stdin.take()
    {
        pipe.write_all(input.as_bytes())
            .map_err(|error| OrbitError::io("write to git stdin", error))?;
    }

    child
        .wait_with_output()
        .map_err(|error| OrbitError::io("wait for git", error))
}

fn git_failure(args: &[&str], output: &Output) -> OrbitError {
    let stderr = String::from_utf8_lossy(&output.stderr).trim().to_string();
    let stdout = String::from_utf8_lossy(&output.stdout).trim().to_string();
    let details = if stderr.is_empty() { stdout } else { stderr };
    let details: String = details.chars().take(2000).collect();
    OrbitError::git(format!(
        "git {} failed: {}",
        args.join(" "),
        if details.is_empty() {
            "unknown error".to_string()
        } else {
            details
        }
    ))
}
