"""changed-areas.sh runs the Rust tests for web files the Rust tests read (P0.3)."""

from pathlib import Path
import re
import subprocess
import tempfile
import unittest


CI_DIR = Path(__file__).resolve().parent
REPO = CI_DIR.parent.parent
SCRIPT = CI_DIR / "changed-areas.sh"


def areas_for(path):
    """Runs the script on a scratch repository whose one commit changes `path`."""
    with tempfile.TemporaryDirectory() as temp:
        root = Path(temp)

        def git(*args):
            subprocess.run(
                ["git", "-c", "user.name=ci", "-c", "user.email=ci@example.invalid", *args],
                cwd=root,
                check=True,
                capture_output=True,
            )

        git("init", "-q")
        (root / "README").write_text("base\n")
        git("add", "README")
        git("commit", "-q", "-m", "base")
        base = subprocess.run(
            ["git", "rev-parse", "HEAD"], cwd=root, check=True, capture_output=True, text=True
        ).stdout.strip()
        target = root / path
        target.parent.mkdir(parents=True, exist_ok=True)
        target.write_text("changed\n")
        git("add", path)
        git("commit", "-q", "-m", "change")
        result = subprocess.run(
            ["bash", str(SCRIPT), base, "HEAD"],
            cwd=root,
            check=True,
            capture_output=True,
            text=True,
        )
        return result.stdout.strip()


def web_paths_the_server_reads():
    """Every `web/src/...` path named in the server's Rust source."""
    found = set()
    for source in (REPO / "apps" / "server" / "src").rglob("*.rs"):
        text = source.read_text()
        for match in re.finditer(r'"[./]*(?:\.\./)+web/(src/[^"]+)"', text):
            found.add("apps/web/" + match.group(1))
    return found


class ChangedAreasTests(unittest.TestCase):
    def test_web_files_rust_reads_run_both(self):
        for path in (
            "apps/web/src/experience/registry.json",
            "apps/web/src/ui/markdown/blockParity.json",
            "apps/web/src/ui/markdown/parseParity.json",
            "apps/web/src/memory/live/pageDocuments.json",
            "apps/web/src/settings/workspace/roles.ts",
        ):
            with self.subTest(path=path):
                self.assertEqual(areas_for(path), "areas=rust,web")

    def test_other_web_files_run_web_only(self):
        self.assertEqual(areas_for("apps/web/src/App.tsx"), "areas=web")

    def test_server_files_run_rust_only(self):
        self.assertEqual(areas_for("apps/server/src/lib.rs"), "areas=rust")

    def test_every_web_file_the_server_reads_is_mapped(self):
        paths = web_paths_the_server_reads()
        self.assertTrue(paths, "found no web paths in the server source")
        for path in sorted(paths):
            with self.subTest(path=path):
                self.assertEqual(
                    areas_for(path),
                    "areas=rust,web",
                    f"{path} is read by a Rust test but changed-areas.sh runs only the web "
                    "checks for it; add it to the shared case",
                )


if __name__ == "__main__":
    unittest.main()
