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
    let cwd = std::env::current_dir()
        .map_err(|error| OrbitError::io("resolve current directory", error))?;
    common_dir_at(&cwd)
}

pub fn common_dir_at(cwd: &Path) -> Result<PathBuf> {
    let output = run_git(
        Some(cwd),
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
        cwd.join(path)
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
            "git user.name and user.email must be configured before creating a Board",
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

/// Adds a detached worktree at `path` using the commit currently checked out by
/// `repo`. Detaching avoids competing with the Board branch while the migration
/// is built and committed.
pub fn worktree_add_detached(repo: &Path, path: &Path) -> Result<()> {
    run_git(
        Some(repo),
        &[
            "worktree",
            "add",
            "--detach",
            path.to_string_lossy().as_ref(),
            "HEAD",
        ],
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

pub fn worktree_prune(repo: &Path) -> Result<()> {
    run_git(Some(repo), &["worktree", "prune"], None)?;
    Ok(())
}

pub fn worktree_paths(repo: &Path) -> Result<Vec<PathBuf>> {
    Ok(worktree_records(repo)?
        .into_iter()
        .map(|record| record.path)
        .collect())
}

#[derive(Debug, Clone)]
pub struct WorktreeRecord {
    pub path: PathBuf,
    pub branch: Option<String>,
    pub missing: bool,
}

/// Parses `git worktree list --porcelain`, including worktrees whose directory
/// disappeared and Git marks as `prunable`.
pub fn worktree_records(repo: &Path) -> Result<Vec<WorktreeRecord>> {
    let output = run_git(Some(repo), &["worktree", "list", "--porcelain"], None)?;
    let mut records = Vec::new();
    let mut path = None;
    let mut branch = None;
    let mut missing = false;

    for line in String::from_utf8_lossy(&output.stdout).lines() {
        if let Some(value) = line.strip_prefix("worktree ") {
            if let Some(path) = path.take() {
                records.push(WorktreeRecord {
                    path,
                    branch: branch.take(),
                    missing,
                });
            }
            path = Some(PathBuf::from(value));
            missing = false;
        } else if let Some(value) = line.strip_prefix("branch ") {
            branch = Some(value.trim_start_matches("refs/heads/").to_string());
        } else if line.starts_with("prunable") {
            missing = true;
        }
    }
    if let Some(path) = path {
        records.push(WorktreeRecord {
            path,
            branch,
            missing,
        });
    }

    for record in &mut records {
        record.missing |= !record.path.exists();
    }
    Ok(records)
}

pub fn worktree_move(repo: &Path, from: &Path, to: &Path) -> Result<()> {
    let original = match run_git(
        Some(repo),
        &[
            "worktree",
            "move",
            from.to_string_lossy().as_ref(),
            to.to_string_lossy().as_ref(),
        ],
        None,
    ) {
        Ok(_) => return Ok(()),
        Err(error) => error,
    };

    // Git for Windows can refuse to rename a worktree out of `.git` with a
    // permission error even when the worktree is clean. Move the directory and
    // repair the worktree metadata instead. Only do this for a clean worktree,
    // so a refusal caused by local changes still stands.
    if !from.exists() {
        if to.exists() {
            repair_moved_worktree(to)?;
            return Ok(());
        }
        return Err(original);
    }
    if to.exists() {
        return Err(original);
    }
    let status = run_git(Some(from), &["status", "--porcelain"], None)?;
    if !status.stdout.is_empty() {
        return Err(original);
    }
    if let Some(parent) = to.parent() {
        std::fs::create_dir_all(parent)
            .map_err(|error| OrbitError::io(format!("create {}", parent.display()), error))?;
    }
    std::fs::rename(from, to).map_err(|error| {
        OrbitError::io(
            format!("move worktree from {} to {}", from.display(), to.display()),
            error,
        )
    })?;
    if let Err(error) = repair_moved_worktree(to) {
        let _ = std::fs::rename(to, from);
        return Err(error);
    }
    Ok(())
}

/// Updates the administrative `gitdir` pointer after a manual worktree move.
/// This is what `git worktree repair` does, without its Windows failure mode of
/// trying to enter the old, now-missing worktree first.
fn repair_moved_worktree(worktree: &Path) -> Result<()> {
    let dot_git = worktree.join(".git");
    let contents = std::fs::read_to_string(&dot_git)
        .map_err(|error| OrbitError::io(format!("read {}", dot_git.display()), error))?;
    let Some(admin) = contents
        .lines()
        .find_map(|line| line.strip_prefix("gitdir: "))
        .map(str::trim)
    else {
        return Err(OrbitError::git(format!(
            "{} is not a linked worktree",
            worktree.display()
        )));
    };
    let admin = Path::new(admin);
    let admin = if admin.is_absolute() {
        admin.to_path_buf()
    } else {
        worktree.join(admin)
    };
    let gitdir = admin.join("gitdir");
    std::fs::write(&gitdir, format!("{}\n", dot_git.display()))
        .map_err(|error| OrbitError::io(format!("write {}", gitdir.display()), error))
}

pub fn exclude_entry_for(repo: &Path, dir: &Path) -> Result<String> {
    let relative = dir.strip_prefix(repo).map_err(|_| {
        OrbitError::config(format!(
            "{} is not inside the repository at {}",
            dir.display(),
            repo.display()
        ))
    })?;
    let relative = relative.to_string_lossy().replace('\\', "/");
    let relative = relative.trim_matches('/');
    if relative.is_empty() {
        return Err(OrbitError::config(
            "the Board directory cannot be the repository root",
        ));
    }
    // Escape gitignore metacharacters and keep trailing spaces significant.
    let mut escaped = String::with_capacity(relative.len() + 2);
    for ch in relative.chars() {
        if matches!(ch, '*' | '?' | '[' | ']' | '\\' | ' ' | '!' | '#') {
            escaped.push('\\');
        }
        escaped.push(ch);
    }
    Ok(format!("/{escaped}/"))
}

/// Adds an untracked exclusion for `dir` to the repository's local exclude file.
/// The path never enters a tracked `.gitignore`.
pub fn ensure_exclude_entry(repo: &Path, dir: &Path) -> Result<()> {
    let pattern = exclude_entry_for(repo, dir)?;

    let info = common_dir_at(repo)?.join("info");
    std::fs::create_dir_all(&info)
        .map_err(|error| OrbitError::io(format!("create {}", info.display()), error))?;
    let path = info.join("exclude");
    let existing = match std::fs::read_to_string(&path) {
        Ok(content) => content,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => String::new(),
        Err(error) => return Err(OrbitError::io(format!("read {}", path.display()), error)),
    };
    if existing.lines().any(|line| line.trim() == pattern) {
        return Ok(());
    }

    let mut content = existing;
    if !content.is_empty() && !content.ends_with('\n') {
        content.push('\n');
    }
    content.push_str(&pattern);
    content.push('\n');
    std::fs::write(&path, content)
        .map_err(|error| OrbitError::io(format!("write {}", path.display()), error))
}

pub fn head_commit(repo: &Path) -> Result<String> {
    let output = run_git(Some(repo), &["rev-parse", "HEAD"], None)?;
    let commit = String::from_utf8_lossy(&output.stdout).trim().to_string();
    if commit.is_empty() {
        return Err(OrbitError::git("git did not report a HEAD commit"));
    }
    Ok(commit)
}

pub fn reset_hard(repo: &Path, commit: &str) -> Result<()> {
    run_git(Some(repo), &["reset", "--hard", commit], None)?;
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

/// Returns the working tree that contains `path`, looking at the nearest
/// existing ancestor when the path itself does not exist yet.
pub fn toplevel_containing(path: &Path) -> Option<PathBuf> {
    let mut candidate = path;
    loop {
        if candidate.exists() {
            break;
        }
        candidate = candidate.parent()?;
    }

    let output = run_git_raw(Some(candidate), &["rev-parse", "--show-toplevel"], None).ok()?;
    if !output.status.success() {
        return None;
    }

    let value = String::from_utf8_lossy(&output.stdout).trim().to_string();
    if value.is_empty() {
        None
    } else {
        Some(PathBuf::from(value))
    }
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn exclude_entry_is_added_once() {
        let repo = tempfile::tempdir().unwrap();
        run_git(Some(repo.path()), &["init", "-q"], None).unwrap();
        let dir = repo.path().join(".orbit");

        ensure_exclude_entry(repo.path(), &dir).unwrap();
        ensure_exclude_entry(repo.path(), &dir).unwrap();

        let exclude =
            std::fs::read_to_string(common_dir_at(repo.path()).unwrap().join("info/exclude"))
                .unwrap();
        assert_eq!(exclude.matches("/.orbit/").count(), 1, "{exclude}");
    }
}
