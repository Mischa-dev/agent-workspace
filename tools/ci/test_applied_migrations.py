"""applied-migrations.sh refuses a change to a migration the base already has."""

from pathlib import Path
import subprocess
import tempfile
import unittest


SCRIPT = Path(__file__).resolve().parent / "applied-migrations.sh"
MIGRATIONS = "apps/server/migrations"
APPLIED = f"{MIGRATIONS}/20261117001400_roles_as_data.sql"


class Branch:
    """A scratch repository whose base has one migration, and a branch off it."""

    def __init__(self, root):
        self.root = Path(root)
        self.git("init", "-q", "-b", "main")
        self.write(APPLIED, "CREATE TABLE roles ();\n")
        self.commit("base")
        self.base = self.git("rev-parse", "HEAD").stdout.strip()
        self.git("checkout", "-q", "-b", "work")

    def git(self, *args):
        return subprocess.run(
            ["git", "-c", "user.name=ci", "-c", "user.email=ci@example.invalid", *args],
            cwd=self.root,
            check=True,
            capture_output=True,
            text=True,
        )

    def write(self, path, text):
        target = self.root / path
        target.parent.mkdir(parents=True, exist_ok=True)
        target.write_text(text)

    def commit(self, message):
        self.git("add", "-A")
        self.git("commit", "-q", "-m", message)

    def check(self):
        return subprocess.run(
            ["bash", str(SCRIPT), self.base, "HEAD"],
            cwd=self.root,
            capture_output=True,
            text=True,
        )


class AppliedMigrationsTests(unittest.TestCase):
    def setUp(self):
        temp = tempfile.TemporaryDirectory()
        self.addCleanup(temp.cleanup)
        self.branch = Branch(temp.name)

    def test_an_edit_to_a_migration_the_base_has_is_refused_with_the_fix(self):
        self.branch.write(APPLIED, "CREATE TABLE roles (name text);\n")
        self.branch.commit("starting roles take a free name")
        result = self.branch.check()
        self.assertEqual(result.returncode, 1)
        self.assertIn(APPLIED, result.stderr)
        self.assertIn(f"git checkout {self.branch.base} -- {APPLIED}", result.stderr)
        self.assertIn("new migration", result.stderr)

    def test_removing_or_renaming_one_is_refused(self):
        self.branch.git("mv", APPLIED, f"{MIGRATIONS}/20261119000000_roles_as_data.sql")
        self.branch.commit("rename")
        result = self.branch.check()
        self.assertEqual(result.returncode, 1)
        self.assertIn(APPLIED, result.stderr)

    def test_turning_one_into_a_link_to_other_sql_is_refused(self):
        self.branch.write("sql/roles.sql", "CREATE TABLE roles (name text);\n")
        applied = self.branch.root / APPLIED
        applied.unlink()
        applied.symlink_to("../../../sql/roles.sql")
        self.branch.commit("share the SQL")
        result = self.branch.check()
        self.assertEqual(result.returncode, 1)
        self.assertIn(APPLIED, result.stderr)

    def test_a_new_migration_may_be_added_and_then_edited(self):
        added = f"{MIGRATIONS}/20261119133117_starting_role_names.sql"
        self.branch.write(added, "ALTER TABLE roles ADD COLUMN name text;\n")
        self.branch.commit("add")
        self.branch.write(added, "ALTER TABLE roles ADD COLUMN name text NOT NULL;\n")
        self.branch.commit("edit the branch's own migration")
        self.assertEqual(self.branch.check().returncode, 0)

    def test_an_edit_put_back_passes(self):
        self.branch.write(APPLIED, "CREATE TABLE roles (name text);\n")
        self.branch.commit("edit")
        self.branch.write(APPLIED, "CREATE TABLE roles ();\n")
        self.branch.commit("put back")
        self.assertEqual(self.branch.check().returncode, 0)

    def test_the_base_moving_on_is_not_this_branchs_change(self):
        self.branch.git("checkout", "-q", "main")
        self.branch.write(f"{MIGRATIONS}/20261120000000_later.sql", "SELECT 1;\n")
        self.branch.commit("main moves on")
        newer_base = self.branch.git("rev-parse", "HEAD").stdout.strip()
        self.branch.git("checkout", "-q", "work")
        self.branch.base = newer_base
        self.assertEqual(self.branch.check().returncode, 0)


if __name__ == "__main__":
    unittest.main()
