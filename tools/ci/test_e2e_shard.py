"""The browser smoke shards share out every test exactly once."""

import json
from pathlib import Path
import subprocess
import sys
import unittest


SCRIPT = Path(__file__).resolve().parent / "e2e_shard.py"


def listing(*files):
    """A `playwright test --list --reporter=json` listing: (file, describes, titles)."""
    suites = []
    for file, describes, titles in files:
        specs = [{"title": t, "file": file, "tests": [{"projectName": "chromium"}]} for t in titles]
        suite = {"title": file, "file": file, "specs": [] if describes else specs, "suites": []}
        for describe in describes:
            suite["suites"].append({"title": describe, "specs": specs, "suites": []})
        suites.append(suite)
    return {"suites": suites}


def shard(data, spec):
    result = subprocess.run(
        [sys.executable, "-I", str(SCRIPT), spec],
        input=json.dumps(data),
        text=True,
        capture_output=True,
        timeout=10,
    )
    return result


class E2eShardTests(unittest.TestCase):
    def test_every_test_lands_in_exactly_one_shard(self):
        data = listing(
            ("slow.spec.ts", ["desktop, light", "phone, dark"], ["sweep one", "sweep two"]),
            ("quick.spec.ts", [], ["a", "b", "c"]),
        )
        lines = []
        for k in (1, 2, 3):
            result = shard(data, f"{k}/3")
            self.assertEqual(result.returncode, 0, result.stderr)
            mine = [l for l in result.stdout.splitlines() if not l.startswith("#")]
            self.assertTrue(mine)
            lines += mine
        self.assertEqual(len(lines), 7)
        self.assertEqual(len(set(lines)), 7)
        self.assertIn("[chromium] › slow.spec.ts › phone, dark › sweep two", lines)
        self.assertIn("[chromium] › quick.spec.ts › b", lines)
        # Neighbours go to different shards.
        first = shard(data, "1/3").stdout
        self.assertIn("desktop, light › sweep one", first)
        self.assertNotIn("desktop, light › sweep two", first)

    def test_a_title_the_list_cannot_carry_fails(self):
        for title in ("a › b", "a > b", " padded", ""):
            with self.subTest(title=title):
                result = shard(listing(("x.spec.ts", [], [title])), "1/1")
                self.assertNotEqual(result.returncode, 0)

    def test_bad_shard_or_empty_listing_fails(self):
        for spec in ("0/3", "4/3", "1of3", ""):
            with self.subTest(spec=spec):
                self.assertNotEqual(shard(listing(("x.spec.ts", [], ["a"])), spec).returncode, 0)
        self.assertNotEqual(shard({"suites": []}, "1/1").returncode, 0)


if __name__ == "__main__":
    unittest.main()
