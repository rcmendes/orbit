//! Board event hooks (`[[hooks]]` in config.toml): local shell commands fired
//! after a write produces feed events. Hooks are best-effort: they run
//! detached, their failure or slowness never affects the write.

mod common;

use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use common::TestRepo;

/// Appends a `[[hooks]]` section to the Board config and commits it so the
/// Board working tree is clean for the command under test.
fn configure_hooks(repo: &TestRepo, rules: &str) {
    let board = repo.board_path();
    let path = board.join("config.toml");
    let mut config = std::fs::read_to_string(&path).unwrap();
    config.push_str(rules);
    std::fs::write(&path, config).unwrap();
    for args in [
        &["add", "-A"][..],
        &["commit", "-q", "-m", "config hooks"][..],
    ] {
        let output = repo.git_in(&board, args);
        assert!(
            output.status.success(),
            "git {args:?} failed: {}",
            String::from_utf8_lossy(&output.stderr)
        );
    }
}

/// Waits up to `timeout` for `path` to exist and contain `needle`; returns the
/// file contents. Polls because hooks run detached after the write returns.
fn wait_for_contents(path: &Path, needle: &str, timeout: Duration) -> Option<String> {
    let deadline = Instant::now() + timeout;
    while Instant::now() < deadline {
        if let Ok(contents) = std::fs::read_to_string(path)
            && contents.contains(needle)
        {
            return Some(contents);
        }
        std::thread::sleep(Duration::from_millis(25));
    }
    std::fs::read_to_string(path).ok()
}

fn hook_log(repo: &TestRepo, name: &str) -> PathBuf {
    repo.path().join(format!("hook-{name}.log"))
}

// Hook commands are POSIX shell and the log path sits in a TOML string.
#[cfg(unix)]
#[test]
fn hook_fires_on_matching_event_with_event_env() {
    let repo = TestRepo::new();
    repo.init();
    let task = repo.add("Hook target", &[]);
    let log = hook_log(&repo, "fire");
    configure_hooks(
        &repo,
        &format!(
            "\n[[hooks]]\nevents = [\"issue.transitioned\"]\ncommand = \"echo \\\"$ORBIT_EVENT_TYPE $ORBIT_EVENT_ISSUE $ORBIT_EVENT_KEY $ORBIT_EVENT_ACTOR\\\" >> '{}'\"\n",
            log.display()
        ),
    );

    repo.ok_json(&["claim", &task, "--by", "hooker", "--json"]);

    let contents = wait_for_contents(&log, "issue.transitioned", Duration::from_secs(10))
        .expect("hook wrote the event line");
    assert!(contents.contains("issue.transitioned"), "{contents}");
    assert!(contents.contains(&task), "{contents}");
    assert!(contents.contains("hooker"), "{contents}");
}

// Hook commands are POSIX shell and the log path sits in a TOML string.
#[cfg(unix)]
#[test]
fn hook_receives_full_event_json() {
    let repo = TestRepo::new();
    repo.init();
    let log = hook_log(&repo, "json");
    configure_hooks(
        &repo,
        &format!(
            "\n[[hooks]]\nevents = [\"*\"]\ncommand = \"echo \\\"$ORBIT_EVENT_JSON\\\" >> '{}'\"\n",
            log.display()
        ),
    );

    repo.add("Created via hook test", &["--reporter", "hook-tester"]);

    let contents = wait_for_contents(&log, "issue.created", Duration::from_secs(10))
        .expect("hook wrote the event JSON");
    let line = contents
        .lines()
        .find(|line| line.contains("issue.created"))
        .expect("an event JSON line");
    let event: serde_json::Value = serde_json::from_str(line).expect("event line is JSON");
    assert_eq!(event["type"], "issue.created");
    assert!(event["id"].as_str().unwrap().contains(':'));
    assert_eq!(event["actor"], "hook-tester");
}

// Hook commands are POSIX shell and the log path sits in a TOML string.
#[cfg(unix)]
#[test]
fn hook_only_fires_for_configured_events() {
    let repo = TestRepo::new();
    repo.init();
    let task = repo.add("Selective hooks", &[]);
    let log = hook_log(&repo, "selective");
    configure_hooks(
        &repo,
        &format!(
            "\n[[hooks]]\nevents = [\"issue.deleted\"]\ncommand = \"echo fired >> '{}'\"\n",
            log.display()
        ),
    );

    repo.ok_json(&["claim", &task, "--by", "hooker", "--json"]);

    // The claim produced issue.transitioned (+ maybe board.changed) but no
    // issue.deleted; the hook must not have fired. Give it ample time.
    std::thread::sleep(Duration::from_secs(1));
    assert!(
        !log.exists(),
        "hook fired for an event it did not subscribe to"
    );
}

#[test]
fn failing_hook_never_fails_the_write() {
    let repo = TestRepo::new();
    repo.init();
    let task = repo.add("Failing hook", &[]);
    configure_hooks(
        &repo,
        "\n[[hooks]]\nevents = [\"*\"]\ncommand = \"exit 1\"\n\n[[hooks]]\nevents = [\"*\"]\ncommand = \"/definitely/missing/hook-binary --oops\"\n",
    );

    // Both hooks fail (nonzero exit, then an unspawnable command); the claim
    // still succeeds.
    let value = repo.ok_json(&["claim", &task, "--by", "hooker", "--json"]);
    assert_eq!(value["data"]["status"], "in_progress");
}

#[test]
fn slow_hook_never_blocks_the_write() {
    let repo = TestRepo::new();
    repo.init();
    let task = repo.add("Slow hook", &[]);
    configure_hooks(
        &repo,
        "\n[[hooks]]\nevents = [\"*\"]\ncommand = \"sleep 30\"\n",
    );

    let start = Instant::now();
    repo.ok_json(&["claim", &task, "--by", "hooker", "--json"]);
    let elapsed = start.elapsed();
    assert!(
        elapsed < Duration::from_secs(15),
        "a detached 30s hook must not block the write; took {elapsed:?}"
    );
}

#[test]
fn hook_config_is_validated() {
    let repo = TestRepo::new();
    repo.init();
    // An empty events list is a configuration error, surfaced on any command.
    configure_hooks(&repo, "\n[[hooks]]\nevents = []\ncommand = \"true\"\n");
    let (_, value) = repo.err_json(&["list", "--json"]);
    let message = value["error"]["message"].as_str().unwrap();
    assert!(message.contains("hooks"), "{message}");

    let repo = TestRepo::new();
    repo.init();
    configure_hooks(
        &repo,
        "\n[[hooks]]\nevents = [\"issue.exploded\"]\ncommand = \"true\"\n",
    );
    let (_, value) = repo.err_json(&["list", "--json"]);
    let message = value["error"]["message"].as_str().unwrap();
    assert!(message.contains("issue.exploded"), "{message}");

    let repo = TestRepo::new();
    repo.init();
    configure_hooks(&repo, "\n[[hooks]]\nevents = [\"*\"]\ncommand = \"  \"\n");
    let (_, value) = repo.err_json(&["list", "--json"]);
    let message = value["error"]["message"].as_str().unwrap();
    assert!(message.contains("command"), "{message}");
}

// Hook commands are POSIX shell and the log path sits in a TOML string.
#[cfg(unix)]
#[test]
fn hooks_do_not_run_unless_the_machine_trusts_them() {
    let repo = TestRepo::new();
    repo.init();
    let task = repo.add("Untrusted hook", &[]);
    let log = hook_log(&repo, "untrusted");
    configure_hooks(
        &repo,
        &format!(
            "\n[[hooks]]\nevents = [\"*\"]\ncommand = \"echo fired >> '{}'\"\n",
            log.display()
        ),
    );

    let mut command = repo.orbit_command(&["claim", &task, "--by", "hooker", "--json"]);
    command.env_remove("ORBIT_TRUST_HOOKS");
    let output = command.output().unwrap();
    assert!(output.status.success());
    assert!(String::from_utf8_lossy(&output.stdout).contains("HOOKS_UNTRUSTED"));
    std::thread::sleep(Duration::from_secs(1));
    assert!(!log.exists(), "an untrusted hook must not run");
}
