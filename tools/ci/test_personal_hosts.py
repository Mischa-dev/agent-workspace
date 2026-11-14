"""The personal_hosts guard (P0.26 rule 8, P0.33): no private-network address
(100.64.0.0/10, the range tailnets hand out) and no `/home/<name>/` path in a
tracked file outside docs/history/, beyond today's allowance per file in
personal_hosts.allowances.json. Allowances only go down; a file under its
allowance passes and says so. Say "<server>" or "<user>@<host>" in docs, and
keep machine addresses in an untracked file (tools/ci/run.sh reads
~/.config/aw-ci/hosts).
"""

import json
import re
import subprocess
import unittest
from pathlib import Path

ROOT = Path(__file__).resolve().parents[2]
ALLOWANCES = Path(__file__).with_name("personal_hosts.allowances.json")
PATTERNS = [
    re.compile(r"(?<![\d.])100\.(?:6[4-9]|[7-9]\d|1[01]\d|12[0-7])\.\d{1,3}\.\d{1,3}(?![\d])"),
    re.compile(r"/home/[a-z_][a-z0-9_-]*/"),
]
SKIPPED_PREFIXES = ("docs/history/",)
SELF = {"tools/ci/test_personal_hosts.py", "tools/ci/personal_hosts.allowances.json"}


def count(text):
    return sum(len(pattern.findall(text)) for pattern in PATTERNS)


def tracked_counts():
    listed = subprocess.run(
        ["git", "ls-files", "-z"], cwd=ROOT, capture_output=True, check=True
    ).stdout.decode().split("\0")
    counts = {}
    for name in filter(None, listed):
        if name.startswith(SKIPPED_PREFIXES) or name in SELF:
            continue
        path = ROOT / name
        if not path.is_file():
            continue
        try:
            text = path.read_text(encoding="utf-8")
        except (UnicodeDecodeError, OSError):
            continue
        found = count(text)
        if found:
            counts[name] = found
    return counts


def judge(counts, allowed):
    over = [
        f"{name} has {found}, allowed {allowed.get(name, 0)}: a machine address or home path; "
        "use a placeholder (AGENTS.md rule 8, \"Nobody's stack is hard-coded\")"
        for name, found in sorted(counts.items())
        if found > allowed.get(name, 0)
    ]
    lower = [
        f"you can lower {name} to {counts.get(name, 0)}"
        for name, allowance in sorted(allowed.items())
        if counts.get(name, 0) < allowance
    ]
    return over, lower


class PersonalHosts(unittest.TestCase):
    def test_the_tree_keeps_within_its_allowances(self):
        allowed = json.loads(ALLOWANCES.read_text())
        self.assertLessEqual(sum(allowed["files"].values()), allowed["ceiling"],
                             "allowances only go down")
        over, lower = judge(tracked_counts(), allowed["files"])
        for hint in lower:
            print(f"personal_hosts: {hint}")
        self.assertEqual(over, [])

    def test_the_patterns(self):
        self.assertEqual(count("ssh ci@100.84.1.2 and http://100.127.255.1:80"), 2)
        self.assertEqual(count("100.63.0.1 100.128.0.1 10.0.0.1 1100.64.0.1"), 0)
        self.assertEqual(count("cd /home/someone/src"), 1)
        self.assertEqual(count("/home/ and /homepage/x"), 0)
        over, lower = judge({"new.md": 1}, {"old.md": 2})
        self.assertEqual(len(over), 1)
        self.assertEqual(lower, ["you can lower old.md to 0"])


if __name__ == "__main__":
    unittest.main()
