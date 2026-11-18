# Benchmarks

## Scan paths vs. the derived index (ADR 0022 gate)

ADR 0022 makes the derived read index conditional: it is built only when a
benchmark on a large Board shows the plain scan too slow. This is that
benchmark — recorded **before** the index was built, then re-measured with
`[index] enabled = true` for comparison.

**Method.** `tests/bench_scan.rs` (ignored in the normal suite) seeds a
synthetic Board by writing N Task files directly plus one Git commit —
Task files are the source of truth, so no Orbit-specific generation is
involved — then times the library scan (`Graph::load` plus the `ready`,
`counts`, and `search` scans) and the user-visible end-to-end latency of
`orbit list`, `orbit ready`, `orbit search`, and `orbit changes`. Each CLI
number is a single timed run after one warm-up. Measured on macOS (Apple
silicon), `--release` build, orbit-tracker 0.10.x:

```sh
cargo test --release --test bench_scan -- --ignored --nocapture
```

### Results

**Library scan** (one `Graph::load` + in-memory queries, warm):

| Tasks | `Graph::load` | `ready_tasks` | `counts` | search (label+text) | search (text only) |
| ---: | ---: | ---: | ---: | ---: | ---: |
| 5,000 | ~155–165 ms | ~21 ms | ~37 ms | ~0.7 ms | ~2.8 ms |
| 10,000 | ~390–400 ms | ~90 ms | ~185 ms | ~1.4 ms | ~6 ms |

**CLI end-to-end, plain scan** (process start, shared lock, load, render):

| Tasks | `list --json` | `ready --json` | `search --json` | `changes --json` |
| ---: | ---: | ---: | ---: | ---: |
| 5,000 | ~375 ms | ~330 ms | ~220 ms | ~55 ms |
| 10,000 | ~1.09 s | ~880 ms | ~410 ms | ~55 ms |

**CLI end-to-end, `[index] enabled` (warm index):**

| Tasks | `list --json` | `ready --json` | `search --json` |
| ---: | ---: | ---: | ---: |
| 5,000 | ~270 ms | ~230 ms | ~115 ms |
| 10,000 | ~775 ms | ~670 ms | ~195 ms |

### Decision

The gate trips: at 10k Tasks `orbit list` and `orbit ready` take ~1 s — past
comfortable interactive latency for a command agents run constantly — and the
cost scales linearly with file count because every read parses every Task
file (`Graph::load` alone is ~400 ms). Query matching itself is already fast
(~1–6 ms); the bottleneck is file I/O plus YAML parsing, which is exactly
what a parse-result cache removes. With the index enabled the same commands
run ~25–55% faster end-to-end (the remaining cost is output rendering and
JSON serialization, which no index can remove).

The index therefore exists, opt-in per `[index] enabled = true`, as a
rebuildable cache under the Board's cache directory — never a source of
truth. See `src/index.rs` and the `orbit index` command.

Threshold rationale for future Boards: below ~5k Tasks the plain scan keeps
reads well under ~400 ms end-to-end and the index is unnecessary overhead;
that is why the module stays opt-in rather than automatic.
