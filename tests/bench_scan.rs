//! Scan-path benchmark for the ADR 0022 "Hooks and derived index" gate.
//!
//! The derived search index is built only when a benchmark on a large Board
//! shows the plain scan too slow. This test is that benchmark: it seeds a
//! synthetic Board with thousands of Issues, then measures the paths an
//! index would accelerate — `Graph::load`, the `ready`/`list`/`search`
//! scans — plus the user-visible end-to-end CLI latency for `orbit list`,
//! `orbit ready`, and `orbit search`.
//!
//! It is `#[ignore]`d so the normal suite stays fast. Run it with:
//!
//! ```sh
//! cargo test --release --test bench_scan -- --ignored --nocapture
//! ```
//!
//! Results are recorded in `docs/benchmarks.md`.

mod common;

use std::fmt::Write as _;
use std::time::Instant;

use common::TestRepo;
use orbit_tracker::board::{AccessMode, Board, BoardAccess};
use orbit_tracker::graph::{Graph, LoadMode};

const WORDS: &[&str] = &[
    "alpha", "bravo", "cache", "delta", "export", "filter", "graph", "hook", "index", "journal",
    "kernel", "ledger", "migrate", "notify", "orbit", "parser", "query", "render", "sync", "token",
];

fn word(i: usize, salt: usize) -> &'static str {
    WORDS[(i * 7 + salt * 13) % WORDS.len()]
}

/// Writes `count` synthetic Task files into the seeded Board and records one
/// commit, mirroring the shape `task::render_task` produces: YAML frontmatter
/// plus a Markdown body. Roughly a fifth of the Tasks carry a `blocks` link
/// and a tenth sit in a terminal status so `ready` has real filtering to do.
fn seed_board(repo: &TestRepo, count: usize) {
    repo.init();
    let board = repo.board_path();
    let tasks = board.join("tasks");
    for i in 0..count {
        let id = format!("orb-{:06}", i + 1);
        let status = match i % 10 {
            0 => "done",
            1 => "in_progress",
            2 => "todo",
            _ => "ready",
        };
        let priority = ["highest", "high", "medium", "low"][i % 4];
        let mut front = String::new();
        writeln!(
            front,
            "---\ntitle: {} {} task {i}\nstatus: {status}\nkey: ORB-{}\ntype: task\npriority: {priority}",
            word(i, 1),
            word(i, 2),
            i + 1
        )
        .unwrap();
        writeln!(front, "labels:\n- {}\n- {}", word(i, 3), word(i, 4)).unwrap();
        if i % 5 == 0 && i > 0 {
            let target = format!("orb-{:06}", i);
            writeln!(front, "links:\n- type: blocks\n  target: {target}").unwrap();
        }
        writeln!(
            front,
            "reporter: bench\ncreated_at: 2026-01-01T00:{:02}:{:02}Z\nupdated_at: 2026-01-02T00:{:02}:{:02}Z\n---\n",
            i % 60,
            (i / 60) % 60,
            i % 60,
            (i / 60) % 60,
        )
        .unwrap();
        let body = format!(
            "Benchmark body for task {i}.\n\nIt mentions {} {} {} and the rare term needle{i}.\n",
            word(i, 5),
            word(i, 6),
            word(i, 7),
        );
        std::fs::write(tasks.join(format!("{id}.md")), format!("{front}\n{body}")).unwrap();
    }
    std::fs::write(
        board.join("keys.toml"),
        format!("next_key = {}\n", count + 1),
    )
    .unwrap();
    for args in [
        &["add", "-A"][..],
        &["commit", "-q", "-m", "bench: seed"][..],
    ] {
        let output = repo.git_in(&board, args);
        assert!(
            output.status.success(),
            "git {args:?} failed: {}",
            String::from_utf8_lossy(&output.stderr)
        );
    }
}

fn open_board(repo: &TestRepo) -> Board {
    BoardAccess::open(Some(repo.path()), AccessMode::Inspect)
        .expect("open Board")
        .into_board()
}

fn bench_library(board: &Board, count: usize) {
    let start = Instant::now();
    let (graph, _) = Graph::load(board, LoadMode::Strict).expect("graph load");
    let load = start.elapsed();
    assert_eq!(graph.tasks.len(), count);

    let start = Instant::now();
    let ready = graph.ready_tasks();
    let ready_time = start.elapsed();

    let start = Instant::now();
    let counts = graph.counts();
    let counts_time = start.elapsed();

    let query = orbit_tracker::search::parse_query("needle1 label:alpha").expect("parse query");
    let start = Instant::now();
    let mut hits: Vec<_> = graph
        .tasks
        .values()
        .filter(|task| orbit_tracker::search::matches(task, &query))
        .collect();
    hits.sort_by_key(|task| orbit_tracker::search::rank(task, &query));
    let search = start.elapsed();

    // A pure free-text scan, the heaviest match path (haystack per task).
    let text = orbit_tracker::search::parse_query("migrate").expect("parse");
    let start = Instant::now();
    let text_hits = graph
        .tasks
        .values()
        .filter(|task| orbit_tracker::search::matches(task, &text))
        .count();
    let text_search = start.elapsed();

    println!(
        "    load={load:?} ready={ready_time:?} ({} ready) counts={counts_time:?} ({counts:?})\n    search[label+text]={search:?} ({} hits) search[text-only]={text_search:?} ({text_hits} hits)",
        ready.len(),
        hits.len()
    );
}

fn bench_cli(repo: &TestRepo) {
    for (name, args) in [
        ("list --json", &["list", "--json"][..]),
        ("ready --json", &["ready", "--json"][..]),
        (
            "search --json",
            &["search", "migrate label:alpha", "--json"][..],
        ),
        ("changes --json", &["changes", "--json"][..]),
    ] {
        // One warm-up run so filesystem caches reflect a steady state.
        repo.orbit(args);
        let start = Instant::now();
        let output = repo.orbit(args);
        let elapsed = start.elapsed();
        assert!(
            output.status.success(),
            "orbit {name} failed: {}",
            String::from_utf8_lossy(&output.stderr)
        );
        println!("    {name}: {elapsed:?}");
    }
}

/// Turns `[index] enabled = true` on for the seeded Board so the benchmark
/// can compare the plain scan against the derived-index read path.
fn enable_index(repo: &TestRepo) {
    let board = repo.board_path();
    let path = board.join("config.toml");
    let mut config = std::fs::read_to_string(&path).unwrap();
    config.push_str("\n[index]\nenabled = true\n");
    std::fs::write(&path, config).unwrap();
    for args in [
        &["add", "-A"][..],
        &["commit", "-q", "-m", "enable index"][..],
    ] {
        let output = repo.git_in(&board, args);
        assert!(
            output.status.success(),
            "git {args:?} failed: {}",
            String::from_utf8_lossy(&output.stderr)
        );
    }
}

fn run(count: usize) {
    let repo = TestRepo::new();
    let start = Instant::now();
    seed_board(&repo, count);
    println!("  seeded {count} tasks in {:?}", start.elapsed());

    let board = open_board(&repo);
    println!("  library scan (Graph::load + queries):");
    bench_library(&board, count);
    // Second pass to show warm-cache behavior.
    bench_library(&board, count);
    println!("  CLI end-to-end (plain scan):");
    bench_cli(&repo);

    enable_index(&repo);
    println!("  CLI end-to-end ([index] enabled, warm):");
    bench_cli(&repo);
}

#[test]
#[ignore = "benchmark; run with `cargo test --release --test bench_scan -- --ignored`"]
fn bench_scan_large_board() {
    for count in [5_000usize, 10_000] {
        println!("== Board with {count} tasks ==");
        run(count);
    }
}
