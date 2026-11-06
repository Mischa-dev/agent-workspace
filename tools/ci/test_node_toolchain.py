"""Keep the build, service and administrative wrapper on the same Node."""

import json
from pathlib import Path
import re
import unittest


ROOT = Path(__file__).resolve().parents[2]


class NodeToolchainTests(unittest.TestCase):
    def test_runtime_and_build_pins_agree(self):
        version = (ROOT / ".node-version").read_text().strip()
        package = json.loads((ROOT / "package.json").read_text())
        self.assertEqual(package["engines"]["node"], version)
        installer = (ROOT / "deploy/redhat/install-toolchains.sh").read_text()
        self.assertEqual(
            re.search(r'^readonly node_version="([^"]+)"$', installer, re.M)[1],
            version,
        )
        for name in (
            "deploy-release.sh",
            "agent-workspace.service",
            "agent-workspace-service-command",
        ):
            with self.subTest(file=name):
                source = (ROOT / "deploy/redhat" / name).read_text()
                self.assertEqual(
                    re.findall(r"node-v([0-9.]+)-linux-arm64/bin", source),
                    [version],
                )
        deploy = (ROOT / "deploy/redhat/deploy-release.sh").read_text()
        self.assertIn(f'!= "v{version}"', deploy)


if __name__ == "__main__":
    unittest.main()
