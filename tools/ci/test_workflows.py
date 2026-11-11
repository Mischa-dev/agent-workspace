"""Keep GitHub Actions workflows off our own machines.

On a public repository anyone's fork pull request starts its
workflows. A job on a self-hosted runner would run that stranger's code on
Mischa's laptop or the Red Hat host, so every job must name a GitHub-hosted
runner, and no workflow may run on pull_request_target (which hands fork code
the repository's token and secrets).
"""

from pathlib import Path
import re
import unittest


WORKFLOWS = Path(__file__).resolve().parents[2] / ".github" / "workflows"
HOSTED = re.compile(r"(ubuntu|windows|macos)-[a-z0-9.]+")


class WorkflowRunnerTests(unittest.TestCase):
    def workflows(self):
        files = sorted(WORKFLOWS.glob("*.yml")) + sorted(WORKFLOWS.glob("*.yaml"))
        self.assertTrue(files, f"no workflows found in {WORKFLOWS}")
        return [(path, path.read_text(encoding="utf-8")) for path in files]

    def test_no_self_hosted_runner(self):
        for path, text in self.workflows():
            with self.subTest(workflow=path.name):
                self.assertNotIn("self-hosted", text)

    def test_every_job_names_a_hosted_runner(self):
        for path, text in self.workflows():
            for line in text.splitlines():
                code = line.split("#", 1)[0].strip()
                if not code.startswith("runs-on:"):
                    continue
                value = code.removeprefix("runs-on:").strip().strip("'\"")
                with self.subTest(workflow=path.name, runs_on=value):
                    self.assertRegex(value, rf"^{HOSTED.pattern}$")

    def test_no_pull_request_target(self):
        for path, text in self.workflows():
            with self.subTest(workflow=path.name):
                self.assertNotIn("pull_request_target", text)


if __name__ == "__main__":
    unittest.main()
