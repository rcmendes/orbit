"""Exercise the actual CI shell step against a temporary Git repository."""

import os
from pathlib import Path
import subprocess
import tempfile
import textwrap
import unittest


WORKFLOW = Path(__file__).resolve().parents[1] / ".github/workflows/ci.yml"


class VersionBumpTest(unittest.TestCase):
    def test_comparison_uses_event_commits(self):
        script = textwrap.dedent(WORKFLOW.read_text().split("        run: |\n")[-1])
        with tempfile.TemporaryDirectory() as directory:
            def git(*args):
                return subprocess.check_output(
                    ["git", "-C", directory, *args], text=True
                ).strip()

            git("init", "-q")
            git("config", "user.name", "CI test")
            git("config", "user.email", "ci@example.invalid")
            manifest = Path(directory) / "Cargo.toml"

            def commit(version):
                manifest.write_text(f'[package]\nversion = "{version}"\n')
                git("add", "Cargo.toml")
                git("commit", "-qm", version)
                return git("rev-parse", "HEAD")

            base = commit("0.12.1")
            for version, expected in [("0.12.2", 0), ("0.12.1", 1), ("0.11.9", 1)]:
                with self.subTest(version=version):
                    head = commit(version)
                    # Main and the merge checkout may change after the event.
                    advanced = commit("0.13.0")
                    git("update-ref", "refs/remotes/origin/main", advanced)
                    result = subprocess.run(
                        ["bash", "-eo", "pipefail", "-c", script],
                        cwd=directory,
                        env={**os.environ, "BASE_REF": "main", "BASE_SHA": base,
                             "HEAD_SHA": head},
                        capture_output=True, text=True,
                    )
                    self.assertEqual(result.returncode, expected, result.stdout + result.stderr)
                    self.assertIn(f"this PR: {version}", result.stdout)


if __name__ == "__main__":
    unittest.main()
