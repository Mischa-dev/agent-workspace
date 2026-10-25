"""Exercise CI area validation without running toolchains or contacting hosts."""

import os
from pathlib import Path
import subprocess
import tempfile
import unittest


CI_DIR = Path(__file__).resolve().parent


class AreaValidationTests(unittest.TestCase):
    def setUp(self):
        self.temp = tempfile.TemporaryDirectory()
        self.addCleanup(self.temp.cleanup)
        self.root = Path(self.temp.name)
        self.bin = self.root / "bin"
        self.bin.mkdir()
        self.calls = self.root / "calls"
        self.env = {
            "PATH": f"{self.bin}{os.pathsep}{os.environ['PATH']}",
            "HOME": str(self.root),
            "LC_ALL": "C",
            "CI_TEST_CALLS": str(self.calls),
            "CI_DATABASE_BASE": "postgres://ci-test",
            "CI_RUN_ID": "area-test",
        }
        # Fail immediately if validation reaches scheduling or repository work.
        for name in ("nice", "git", "ssh", "rsync"):
            self.stub(self.bin / name, status=97)
        for name in ("cargo", "rustup", "node", "pnpm", "python3", "psql", "createdb", "dropdb"):
            self.stub(self.bin / name)
        self.stub(self.root / "tools" / "verify-self-hosted-guide.sh")
        # Keep combined checks sequential for deterministic call logs.
        self.stub(self.bin / "nproc", output="4")

    def stub(self, path, status=0, output=""):
        path.parent.mkdir(parents=True, exist_ok=True)
        path.write_text(
            '#!/bin/sh\n'
            'printf "%s" "${0##*/}" >> "$CI_TEST_CALLS"\n'
            'printf " %s" "$@" >> "$CI_TEST_CALLS"\n'
            'printf "\\n" >> "$CI_TEST_CALLS"\n'
            f'printf "%s" "{output}"\n'
            f'exit {status}\n'
        )
        path.chmod(0o755)

    def run_script(self, name, args, scoped=False):
        self.calls.write_text("")
        env = dict(self.env)
        if scoped:
            env["CI_SCOPED"] = "1"
        result = subprocess.run(
            ["bash", str(CI_DIR / name), *args],
            cwd=self.root,
            env=env,
            text=True,
            capture_output=True,
            timeout=10,
        )
        return result, self.calls.read_text().splitlines()

    def test_invalid_areas_fail_before_side_effects(self):
        for areas in ("rst", "rust,rst", "", "rust,,web", "rust,"):
            for name, args in (
                ("checks.sh", [areas]),
                ("run.sh", ["--areas", areas]),
            ):
                with self.subTest(script=name, areas=areas):
                    result, calls = self.run_script(name, args)
                    self.assertEqual(result.returncode, 2, result.stdout + result.stderr)
                    self.assertNotIn("All checks passed", result.stdout)
                    self.assertEqual(calls, [], "invalid input reached external tooling")

    def test_checks_dispatch_documented_areas_and_default(self):
        for args, rust, web in (
            ([], True, True),
            (["rust,web"], True, True),
            (["rust"], True, False),
            (["web"], False, True),
        ):
            with self.subTest(args=args):
                result, calls = self.run_script("checks.sh", args, scoped=True)
                self.assertEqual(result.returncode, 0, result.stdout + result.stderr)
                self.assertIn("All checks passed", result.stdout)
                self.assertEqual("cargo fmt --check" in calls, rust)
                self.assertEqual("cargo nextest run --workspace --profile ci" in calls, rust)
                self.assertEqual("pnpm lint" in calls, web)
                self.assertEqual("pnpm build" in calls, web)

    def test_node_check_failure_stops_before_dependency_install(self):
        self.stub(self.bin / "node", status=97)
        result, calls = self.run_script("checks.sh", ["web"], scoped=True)
        self.assertEqual(result.returncode, 97, result.stdout + result.stderr)
        self.assertIn("Node version failed", result.stdout)
        self.assertFalse(any(call.startswith("pnpm ") for call in calls))

    def test_runner_accepts_documented_areas_and_default(self):
        for args in ([], ["--areas", "rust,web"], ["--areas", "rust"], ["--areas", "web"]):
            with self.subTest(args=args):
                result, calls = self.run_script("run.sh", args)
                self.assertEqual(result.returncode, 97, result.stdout + result.stderr)
                self.assertEqual(calls, ["git rev-parse --show-toplevel"])


if __name__ == "__main__":
    unittest.main()

