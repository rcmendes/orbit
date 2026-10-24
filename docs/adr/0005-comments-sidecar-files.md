# Comments live in sidecar files

Comments are append-only and stored per task at `comments/<task-id>.md`, using marker-delimited blocks with an author and RFC 3339 timestamp. The task file stays focused on description and acceptance criteria; `orbit show` merges both.

Putting comments in the task body would require a reserved section that user Markdown can collide with. A `comments` array in frontmatter would bloat the metadata and make every task read/write carry conversation history. A file per comment would be more merge-friendly but multiply board files. The sidecar keeps one comments file per contributed task, is easy to parse for triage, and is written under the same board lock.

The cost is a second file for commented tasks and a delete cascade: `orbit delete` removes the sidecar, and `doctor` reports malformed comments and orphan comment files.
