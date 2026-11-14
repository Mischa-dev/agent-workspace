"""Relative links in the front-door documents resolve (P0.30, P0.31).

Checks README.md, AGENTS.md, the PROJECT_SOURCE_OF_TRUTH.md stub and every
Markdown file under docs/plans/master/. Links to the web, anchors and
mail addresses are left alone; a link to a file or folder in the repository
must exist. Run by checks.sh with the other CI script tests.
"""

import re
import unittest
from pathlib import Path

ROOT = Path(__file__).resolve().parents[2]
LINK = re.compile(r"\[[^\]]*\]\(([^)\s]+)(?:\s+\"[^\"]*\")?\)")


def documents():
    yield ROOT / "README.md"
    yield ROOT / "AGENTS.md"
    yield ROOT / "PROJECT_SOURCE_OF_TRUTH.md"
    yield from sorted((ROOT / "docs" / "plans" / "master").rglob("*.md"))


def broken_links(path):
    text = path.read_text(encoding="utf-8")
    # Code blocks and inline code hold examples, not links.
    text = re.sub(r"```.*?```", "", text, flags=re.S)
    text = re.sub(r"`[^`\n]*`", "", text)
    for target in LINK.findall(text):
        if re.match(r"^[a-z][a-z0-9+.-]*:", target) or target.startswith("#"):
            continue
        target = target.split("#", 1)[0]
        if not target:
            continue
        resolved = ROOT / target.lstrip("/") if target.startswith("/") else path.parent / target
        if not resolved.exists():
            yield target


class DocsLinks(unittest.TestCase):
    def test_relative_links_resolve(self):
        problems = []
        for path in documents():
            if not path.exists():
                problems.append(f"{path.relative_to(ROOT)} is missing")
                continue
            for target in broken_links(path):
                problems.append(f"{path.relative_to(ROOT)}: {target}")
        self.assertEqual(problems, [])

    def test_a_broken_link_is_found(self):
        sample = ROOT / "tools" / "ci" / "__docs_links_sample__.md"
        try:
            sample.write_text("[gone](no-such-file.md) [web](https://example.com) [here](#top)\n")
            self.assertEqual(list(broken_links(sample)), ["no-such-file.md"])
        finally:
            sample.unlink(missing_ok=True)


if __name__ == "__main__":
    unittest.main()
