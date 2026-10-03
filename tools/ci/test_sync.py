"""Exercise source sync locally with Git and rsync, without contacting CI hosts."""

import os
from pathlib import Path
import subprocess
import tempfile
import unittest


RUN_SCRIPT = Path(__file__).with_name("run.sh")


class SourceSyncTests(unittest.TestCase):
    def setUp(self):
        self.temp = tempfile.TemporaryDirectory()
        self.addCleanup(self.temp.cleanup)
        self.root = Path(self.temp.name)
        self.source = self.root / "source"
        self.source.mkdir()
        self.work = self.root / "runner"
        self.destination = self.work / "src"
        self.git("init", "-q")
        self.write(".gitignore", ".env.*\n!.env.example\n*.log\nsecrets/\ntarget/\n")

    def git(self, *args):
        subprocess.run(["git", *args], cwd=self.source, check=True, capture_output=True)

    def write(self, name, contents="source"):
        path = self.source / name
        path.parent.mkdir(parents=True, exist_ok=True)
        path.write_text(contents)

    def sync(self):
        # Load the actual implementation, stopping before host scheduling starts.
        script, boundary, _ = RUN_SCRIPT.read_text().partition("# One CI job per machine:")
        self.assertTrue(boundary, "could not isolate sync from host scheduling")
        script += '\ntransport=local\nwork="$TEST_SYNC_WORK"\nsync_source\n'
        result = subprocess.run(
            ["bash"], input=script, text=True, cwd=self.source,
            env={**os.environ, "TEST_SYNC_WORK": str(self.work)}, capture_output=True,
            timeout=10,
        )
        self.assertEqual(result.returncode, 0, result.stdout + result.stderr)

    def test_sync_uses_git_ignore_semantics_and_working_tree_contents(self):
        self.write("nested/.gitignore", "*.config\n!important.config\n")
        included = [
            ".env.example", "nested/important.config", "tracked.log",
            "secrets/tracked", "scratch1XY.log", "modified.txt",
        ]
        for name in included:
            self.write(name)
        self.git("add", "-f", ".gitignore", "nested/.gitignore", *included)
        self.write("modified.txt", "uncommitted edit")
        untracked = ["untracked source.txt", "line\nbreak.txt"]
        ignored = [
            ".env.secret", "nested/private.config", "secrets/private",
            "scratch[1]*?.log", "space name.log", "line\nbreak.log",
            "back\\slash.log", "back\\slash[1].log", "target/cache",
            "node_modules/dependency", "dist/output",
        ]
        for name in untracked + ignored:
            self.write(name)

        self.sync()

        for name in included + untracked:
            with self.subTest(included=name):
                self.assertTrue((self.destination / name).is_file())
                self.assertEqual(
                    (self.destination / name).read_bytes(), (self.source / name).read_bytes()
                )
        for name in ignored + [".git"]:
            with self.subTest(excluded=name):
                self.assertFalse((self.destination / name).exists())

    def test_sync_deletes_removed_sources_and_keeps_warm_caches(self):
        self.write("tracked.txt")
        self.write("untracked.txt")
        self.git("add", "tracked.txt")
        self.sync()
        (self.source / "tracked.txt").unlink()
        (self.source / "untracked.txt").unlink()
        for directory in ["target", "node_modules", "dist"]:
            cache = self.destination / directory / "warm"
            cache.parent.mkdir()
            cache.write_text("cached")

        self.sync()

        for name in ["tracked.txt", "untracked.txt"]:
            self.assertFalse((self.destination / name).exists())
        for directory in ["target", "node_modules", "dist"]:
            self.assertEqual((self.destination / directory / "warm").read_text(), "cached")


if __name__ == "__main__":
    unittest.main()
