"""The opt-in CI UI tour (run.sh --host redhat --ui-tour channel), without
running toolchains, browsers or contacting hosts."""

import importlib.util
import json
import os
from pathlib import Path
import subprocess
import tempfile
import time
import unittest


CI_DIR = Path(__file__).resolve().parent
TOUR_CALL = "pnpm --filter @agent-workspace/web exec playwright test --config playwright.tour.config.ts"
SCREENS = [f"02-channel-{size}-{scheme}.png" for size in ("desktop", "phone") for scheme in ("light", "dark")]


class Harness(unittest.TestCase):
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
            "CI_RUN_ID": "tour-test",
            "PLAYWRIGHT_CHROMIUM_EXECUTABLE": str(self.root / "chromium"),
        }
        # Anything reaching scheduling, the repository or an unscoped run fails loudly.
        for name in ("nice", "git", "ssh", "rsync"):
            self.stub(self.bin / name, status=97)
        for name in ("cargo", "rustup", "node", "python3", "psql", "createdb", "dropdb"):
            self.stub(self.bin / name)
        self.stub(self.root / "target" / "debug" / "agent-workspace-server")
        self.stub(self.bin / "nproc", output="4")
        # pnpm notes what the tour would see, and fails where asked to.
        self.pnpm(fail_on="")

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

    def pnpm(self, fail_on):
        path = self.bin / "pnpm"
        path.write_text(
            '#!/bin/sh\n'
            'printf "pnpm" >> "$CI_TEST_CALLS"\n'
            'printf " %s" "$@" >> "$CI_TEST_CALLS"\n'
            'printf "\\n" >> "$CI_TEST_CALLS"\n'
            'case "$*" in test:e2e|*playwright.tour.config.ts*)\n'
            '  echo "$* | only=${UI_TOUR_ONLY-} db=${E2E_DATABASE_NAME-} server=${E2E_SERVER_BINARY-}" >> "$CI_TEST_CALLS.env" ;;\n'
            'esac\n'
            f'case "$*" in {fail_on or "__never__"}) exit 1 ;; esac\n'
            'exit 0\n'
        )
        path.chmod(0o755)

    def run_script(self, name, args, scoped=True, **env):
        self.calls.write_text("")
        Path(f"{self.calls}.env").write_text("")
        full = {**self.env, **env}
        if scoped:
            full["CI_SCOPED"] = "1"
        result = subprocess.run(
            ["bash", str(CI_DIR / name), *args],
            cwd=self.root, env=full, text=True, capture_output=True, timeout=10,
        )
        return result, self.calls.read_text().splitlines()

    def seen(self):
        return Path(f"{self.calls}.env").read_text().splitlines()


class RunnerTests(Harness):
    def test_the_tour_needs_its_one_name_and_red_hat_by_name(self):
        for args in (
            ["--ui-tour", "channel"],
            ["--ui-tour", "channel", "--host", "nixos"],
            ["--host", "redhat", "--ui-tour", "dock"],
            ["--host", "redhat", "--ui-tour", ""],
            ["--host", "redhat", "--ui-tour"],
        ):
            with self.subTest(args=args):
                result, calls = self.run_script("run.sh", args)
                self.assertEqual(result.returncode, 2, result.stdout + result.stderr)
                self.assertEqual(calls, [], "a refused tour reached external tooling")

    def test_red_hat_by_name_gets_past_validation(self):
        result, calls = self.run_script("run.sh", ["--host", "redhat", "--areas", "web", "--ui-tour", "channel"])
        self.assertEqual(result.returncode, 97, result.stdout + result.stderr)
        self.assertEqual(calls, ["git rev-parse --show-toplevel"])

    def command(self, *args):
        """checks_command as run.sh builds it for Red Hat, from run.sh itself."""
        script, boundary, _ = (CI_DIR / "run.sh").read_text().partition("# One CI job per machine:")
        self.assertTrue(boundary, "could not isolate checks_command from host scheduling")
        repo = Path(tempfile.mkdtemp(dir=self.root))
        subprocess.run(["/usr/bin/env", "git", "init", "-q"], cwd=repo, check=True,
                       env={**os.environ}, capture_output=True)
        script += '\nload_host redhat\nchecks_command\n'
        result = subprocess.run(["bash", "-s", "--", *args], input=script, text=True, cwd=repo,
                                env={**os.environ}, capture_output=True, timeout=10)
        self.assertEqual(result.returncode, 0, result.stdout + result.stderr)
        return result.stdout

    def test_only_the_tour_option_forwards_the_tour(self):
        asked = self.command("--host", "redhat", "--areas", "web", "--ui-tour", "channel")
        self.assertIn("CI_UI_TOUR='channel'", asked)
        self.assertIn("tools/ci/checks.sh 'web'", asked)
        for args in (("--host", "redhat", "--areas", "web"), ("--areas", "web"), ()):
            with self.subTest(args=args):
                self.assertNotIn("CI_UI_TOUR", self.command(*args))


class ChecksTests(Harness):
    def test_off_by_default(self):
        result, calls = self.run_script("checks.sh", ["web"])
        self.assertEqual(result.returncode, 0, result.stdout + result.stderr)
        self.assertIn("pnpm test:e2e", calls)
        self.assertNotIn(TOUR_CALL, calls)
        self.assertFalse(any("check_ui_tour.py" in call for call in calls), calls)

    def test_after_the_smoke_tests_with_their_server_and_database(self):
        result, calls = self.run_script("checks.sh", ["web"], CI_UI_TOUR="channel")
        self.assertEqual(result.returncode, 0, result.stdout + result.stderr)
        smoke, tour = calls.index("pnpm test:e2e"), calls.index(TOUR_CALL)
        self.assertLess(smoke, tour)
        checked = [call for call in calls if "check_ui_tour.py" in call]
        self.assertEqual(len(checked), 1, calls)
        self.assertTrue(checked[0].startswith("python3 tools/ci/check_ui_tour.py apps/web/e2e-output "), checked)
        self.assertLess(tour, calls.index(checked[0]))
        smoke_env, tour_env = self.seen()
        server = str(self.root / "target" / "debug" / "agent-workspace-server")
        self.assertEqual(smoke_env, f"test:e2e | only= db=aw_e2e_tour_test server={server}")
        self.assertEqual(tour_env, f"{TOUR_CALL[5:]} | only=channel db=aw_e2e_tour_test server={server}")
        # No second server build for the tour.
        builds = [call for call in calls if call.startswith("cargo build")]
        self.assertLessEqual(len(builds), 1, builds)

    def test_a_smoke_failure_still_fails_and_skips_the_tour(self):
        self.pnpm(fail_on="test:e2e")
        result, calls = self.run_script("checks.sh", ["web"], CI_UI_TOUR="channel")
        self.assertNotEqual(result.returncode, 0, result.stdout + result.stderr)
        self.assertIn("web browser smoke failed", result.stdout)
        self.assertNotIn(TOUR_CALL, calls)
        self.assertNotIn("All checks passed", result.stdout)

    def test_a_tour_failure_fails_the_step(self):
        self.pnpm(fail_on="*playwright.tour.config.ts*")
        result, calls = self.run_script("checks.sh", ["web"], CI_UI_TOUR="channel")
        self.assertNotEqual(result.returncode, 0, result.stdout + result.stderr)
        self.assertIn("web browser smoke failed", result.stdout)
        self.assertFalse(any("check_ui_tour.py" in call for call in calls), calls)
        self.assertNotIn("All checks passed", result.stdout)

    def test_a_failed_tour_check_fails_the_step(self):
        (self.bin / "python3").write_text(
            '#!/bin/sh\nprintf "python3 %s\\n" "$*" >> "$CI_TEST_CALLS"\n'
            'case "$*" in *check_ui_tour.py*) exit 1 ;; esac\nexit 0\n'
        )
        result, _ = self.run_script("checks.sh", ["web"], CI_UI_TOUR="channel")
        self.assertNotEqual(result.returncode, 0, result.stdout + result.stderr)
        self.assertIn("web browser smoke failed", result.stdout)

    def test_an_unknown_tour_is_refused_before_anything_runs(self):
        result, calls = self.run_script("checks.sh", ["web"], scoped=False, CI_UI_TOUR="dock")
        self.assertEqual(result.returncode, 2, result.stdout + result.stderr)
        self.assertEqual(calls, [])

    def test_the_tour_never_runs_uncapped(self):
        self.stub(self.bin / "systemd-run", status=1)
        for env in ({"CI_MEMORY_MAX": "10G"}, {}):
            with self.subTest(env=env):
                result, calls = self.run_script("checks.sh", ["web"], scoped=False, CI_UI_TOUR="channel", **env)
                self.assertEqual(result.returncode, 125, result.stdout + result.stderr)
                self.assertIn("the UI tour needs a capped scope", result.stderr)
                self.assertFalse(any(call.startswith(("nice", "pnpm")) for call in calls), calls)
        # Without the tour a machine without user scopes still runs at low priority, as before.
        result, calls = self.run_script("checks.sh", ["web"], scoped=False, CI_MEMORY_MAX="10G")
        self.assertEqual(result.returncode, 97, result.stdout + result.stderr)
        self.assertTrue(any(call.startswith("nice -n 10") for call in calls), calls)


def load_checker():
    spec = importlib.util.spec_from_file_location("check_ui_tour", CI_DIR / "check_ui_tour.py")
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    return module


class TourCheckTests(unittest.TestCase):
    def setUp(self):
        self.checker = load_checker()
        self.temp = tempfile.TemporaryDirectory()
        self.addCleanup(self.temp.cleanup)
        self.output = Path(self.temp.name)
        (self.output / "state").mkdir()
        (self.output / "tour").mkdir()
        self.started = int(time.time()) - 5
        self.state(runId="019fb8b0-0000-7000-8000-0000000000aa", warnings=[])
        for name in SCREENS:
            (self.output / "tour" / name).write_bytes(b"\x89PNG\r\n\x1a\n" + name.encode())
        (self.output / "tour" / "index.html").write_text("".join(f'<img src="{name}">' for name in SCREENS))
        # The signed-in browser state sits next to it and must never be summarised.
        (self.output / "state" / "signed-in.json").write_text('{"cookies": [{"value": "session-secret"}]}')

    def state(self, **fields):
        base = {"projectId": "p", "channelLastMessage": "seeded words", "runId": None, "warnings": []}
        (self.output / "state" / "tour.json").write_text(json.dumps({**base, **fields}))

    def run_check(self):
        return self.checker.check(self.output, self.started)

    def test_a_complete_tour_passes_and_summarises_without_secrets_or_seeded_text(self):
        problems, _ = self.run_check()
        self.assertEqual(problems, [])
        self.assertEqual(self.checker.main(["check", str(self.output), str(self.started)]), 0)
        summary = (self.output / "tour" / "summary.txt").read_text()
        self.assertIn("runId: 019fb8b0-0000-7000-8000-0000000000aa", summary)
        for name in SCREENS:
            self.assertIn(name, summary)
        self.assertNotIn("session-secret", summary)
        self.assertNotIn("seeded words", summary)

    def test_no_run_fails(self):
        self.state(runId=None, warnings=[])
        problems, _ = self.run_check()
        self.assertTrue(any("runId is null" in problem for problem in problems), problems)

    def test_a_seed_warning_fails(self):
        self.state(runId="r", warnings=["no decision recorded (403)"])
        problems, _ = self.run_check()
        self.assertTrue(any("warned 1 time" in problem for problem in problems), problems)
        self.state(runId="r")
        state = json.loads((self.output / "state" / "tour.json").read_text())
        del state["warnings"]
        (self.output / "state" / "tour.json").write_text(json.dumps(state))
        problems, _ = self.run_check()
        self.assertTrue(any("doesn't say what it warned" in problem for problem in problems), problems)

    def test_missing_stale_or_wrong_images_fail(self):
        (self.output / "tour" / SCREENS[0]).unlink()
        old = self.started - 60
        os.utime(self.output / "tour" / SCREENS[1], (old, old))
        (self.output / "tour" / SCREENS[2]).write_bytes(b"not a png")
        problems, _ = self.run_check()
        self.assertIn(f"missing {SCREENS[0]}", problems)
        self.assertIn(f"{SCREENS[1]} is from before this run", problems)
        self.assertIn(f"{SCREENS[2]} isn't a PNG", problems)
        self.assertEqual(self.checker.main(["check", str(self.output), str(self.started)]), 1)

    def test_a_contact_sheet_missing_a_screen_fails(self):
        (self.output / "tour" / "index.html").write_text(f'<img src="{SCREENS[0]}">')
        problems, _ = self.run_check()
        self.assertIn(f"index.html doesn't show {SCREENS[3]}", problems)

    def test_no_seed_state_fails(self):
        (self.output / "state" / "tour.json").unlink()
        problems, _ = self.run_check()
        self.assertTrue(problems and problems[0].startswith("no tour seed state"), problems)


if __name__ == "__main__":
    unittest.main()
