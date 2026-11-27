"""tools/git-hooks/pre-commit refuses a commit whose Rust isn't formatted."""

from pathlib import Path
import shutil
import subprocess
import tempfile
import unittest


REPO = Path(__file__).resolve().parents[2]
HOOK = REPO / "tools" / "git-hooks" / "pre-commit"
FORMATTED = "pub fn one() -> u8 {\n    1\n}\n"
UNFORMATTED = "pub fn one() -> u8 { 1 }\n"


def has_rustfmt():
    if not shutil.which("rustfmt"):
        return False
    return subprocess.run(["rustfmt", "--version"], cwd=REPO, capture_output=True).returncode == 0


@unittest.skipUnless(has_rustfmt(), "needs rustfmt")
class PreCommitTests(unittest.TestCase):
    def setUp(self):
        temp = tempfile.TemporaryDirectory()
        self.addCleanup(temp.cleanup)
        self.root = Path(temp.name)
        self.git("init", "-q")
        self.write("Cargo.toml", '[package]\nname = "scratch"\nversion = "0.0.0"\nedition = "2024"\n')
        shutil.copy(REPO / "rust-toolchain.toml", self.root)
        self.write("src/lib.rs", FORMATTED)
        self.write("notes.md", "one\n")

    def git(self, *args):
        subprocess.run(["git", *args], cwd=self.root, check=True, capture_output=True)

    def write(self, path, text):
        target = self.root / path
        target.parent.mkdir(parents=True, exist_ok=True)
        target.write_text(text)

    def commit_check(self):
        return subprocess.run(["bash", str(HOOK)], cwd=self.root, capture_output=True, text=True)

    def test_unformatted_rust_is_refused_with_the_fix(self):
        self.write("src/lib.rs", UNFORMATTED)
        self.git("add", "-A")
        result = self.commit_check()
        self.assertEqual(result.returncode, 1)
        self.assertIn("  src/lib.rs", result.stderr)
        self.assertIn("rustfmt --edition 2024 src/lib.rs && git add src/lib.rs && git commit", result.stderr)

    def test_formatted_rust_is_committed(self):
        self.git("add", "-A")
        self.assertEqual(self.commit_check().returncode, 0)

    def test_what_is_checked_is_what_the_commit_holds(self):
        self.write("src/lib.rs", UNFORMATTED)
        self.git("add", "-A")
        self.write("src/lib.rs", FORMATTED)
        self.assertEqual(self.commit_check().returncode, 1)

        self.git("add", "-A")
        self.write("src/lib.rs", UNFORMATTED)
        self.assertEqual(self.commit_check().returncode, 0)

    def test_a_file_whose_modules_are_elsewhere_is_checked_alone(self):
        self.write("src/lib.rs", "mod other;\n" + FORMATTED)
        self.write("src/other.rs", UNFORMATTED)
        self.git("add", "src/lib.rs", "Cargo.toml")
        self.assertEqual(self.commit_check().returncode, 0)

    def test_a_commit_without_rust_is_not_checked(self):
        self.git("add", "-A")
        self.git("-c", "user.name=ci", "-c", "user.email=ci@example.invalid", "commit", "-q", "-m", "base")
        self.write("src/lib.rs", UNFORMATTED)
        self.write("notes.md", "two\n")
        self.git("add", "notes.md")
        self.assertEqual(self.commit_check().returncode, 0)


if __name__ == "__main__":
    unittest.main()
