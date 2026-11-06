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
            "PLAYWRIGHT_CHROMIUM_EXECUTABLE": str(self.root / "chromium"),
        }
        # Fail immediately if validation reaches scheduling or repository work.
        for name in ("nice", "git", "ssh", "rsync"):
            self.stub(self.bin / name, status=97)
        for name in ("cargo", "rustup", "node", "pnpm", "python3", "psql", "createdb", "dropdb"):
            self.stub(self.bin / name)
        self.stub(self.root / "tools" / "verify-self-hosted-guide.sh")
        self.stub(self.root / "tools" / "ci" / "test_admission.sh")
        self.stub(self.root / "tools" / "ci" / "test_pinned_node.sh")
        self.stub(self.root / "target" / "debug" / "agent-workspace-server")
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
                self.assertEqual(any(c.split() == ["test_admission.sh"] for c in calls), rust)
                self.assertEqual(any(c.split() == ["test_pinned_node.sh"] for c in calls), rust)
                self.assertEqual("cargo nextest run --workspace --profile ci" in calls, rust)
                self.assertEqual("pnpm lint" in calls, web)
                self.assertEqual("pnpm build" in calls, web)
                # The browser smoke tests drive the server too, so they run
                # for a change on either side.
                self.assertIn("pnpm install --frozen-lockfile --reporter=silent", calls)
                self.assertIn("pnpm test:e2e", calls)

    def test_browser_smoke_fails_in_ci_instead_of_skipping(self):
        del self.env["PLAYWRIGHT_CHROMIUM_EXECUTABLE"]
        result, calls = self.run_script("checks.sh", ["web"], scoped=True)
        self.assertNotEqual(result.returncode, 0, result.stdout + result.stderr)
        self.assertIn("Browser smoke tests can't run: no Chromium", result.stdout + result.stderr)
        self.assertNotIn("All checks passed", result.stdout)
        self.assertNotIn("pnpm test:e2e", calls)

        # Outside a CI run, a missing browser is a skip with a note.
        del self.env["CI_RUN_ID"]
        result, calls = self.run_script("checks.sh", ["web"], scoped=True)
        self.assertEqual(result.returncode, 0, result.stdout + result.stderr)
        self.assertIn("Skipping browser smoke tests: no Chromium", result.stdout)
        self.assertIn("All checks passed", result.stdout)

    def test_node_check_failure_stops_before_dependency_install(self):
        self.stub(self.bin / "node", status=97)
        result, calls = self.run_script("checks.sh", ["web"], scoped=True)
        self.assertEqual(result.returncode, 97, result.stdout + result.stderr)
        self.assertIn("Node version failed", result.stdout)
        self.assertFalse(any(call.startswith("pnpm ") for call in calls))

    def test_host_on_the_pinned_node_fetches_nothing(self):
        (self.root / ".node-version").write_text("24.22.1\n")
        self.stub(self.bin / "node", output="v24.22.1")
        self.stub(self.bin / "curl", status=97)
        result, calls = self.run_script("checks.sh", ["web"], scoped=True)
        self.assertFalse(any(call.startswith("curl ") for call in calls), calls)
        self.assertNotIn("Node toolchain failed", result.stdout)

    def test_host_on_another_node_fetches_the_pin_or_stops(self):
        (self.root / ".node-version").write_text("24.22.1\n")
        self.stub(self.bin / "node", output="v22.22.2")
        self.stub(self.bin / "curl", status=22)
        result, calls = self.run_script("checks.sh", ["web"], scoped=True)
        self.assertNotEqual(result.returncode, 0, result.stdout + result.stderr)
        self.assertIn("Node toolchain failed", result.stdout)
        self.assertTrue(any("nodejs.org/dist/v24.22.1/" in call for call in calls), calls)
        self.assertFalse(any(call.startswith("pnpm ") for call in calls))

    def test_runner_accepts_documented_areas_and_default(self):
        for args in ([], ["--areas", "rust,web"], ["--areas", "rust"], ["--areas", "web"]):
            with self.subTest(args=args):
                result, calls = self.run_script("run.sh", args)
                self.assertEqual(result.returncode, 97, result.stdout + result.stderr)
                self.assertEqual(calls, ["git rev-parse --show-toplevel"])


if __name__ == "__main__":
    unittest.main()

