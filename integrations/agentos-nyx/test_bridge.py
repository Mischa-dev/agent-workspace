"""Focused checks for the Nyx bridge's operator preflight."""

import contextlib
import importlib.util
import io
import json
from pathlib import Path
import sys
import tempfile
import unittest
from unittest.mock import patch


SPEC = importlib.util.spec_from_file_location("aw_bridge", Path(__file__).with_name("bridge.py"))
bridge = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(bridge)


class CheckTests(unittest.TestCase):
    def setUp(self):
        temporary = tempfile.TemporaryDirectory()
        self.addCleanup(temporary.cleanup)
        self.state = Path(temporary.name)
        (self.state / "agents.json").write_text(json.dumps({"mara": {"agentos_id": "mara"}}))

    def test_check_verifies_inbox_and_agentos_controller(self):
        output = io.StringIO()
        with (
            patch.object(bridge, "STATE", self.state),
            patch.object(bridge, "Workspace") as workspace_type,
            patch.object(bridge, "controller", return_value={"source_revision": "sha256:example"}) as controller_call,
            patch.object(sys, "argv", ["bridge.py", "--agent", "mara", "--check"]),
            contextlib.redirect_stdout(output),
        ):
            workspace_type.return_value.inbox.return_value = [{"requestId": "request"}]
            bridge.main()

        workspace_type.return_value.connect.assert_called_once_with()
        workspace_type.return_value.inbox.assert_called_once_with()
        controller_call.assert_called_once_with({"operation": "plan"})
        self.assertIn("AgentOS controller reachable", output.getvalue())

    def test_check_fails_when_controller_identity_cannot_be_verified(self):
        with (
            patch.object(bridge, "STATE", self.state),
            patch.object(bridge, "Workspace") as workspace_type,
            patch.object(bridge, "controller", side_effect=RuntimeError("controller_peer_mismatch")),
            patch.object(sys, "argv", ["bridge.py", "--agent", "mara", "--check"]),
        ):
            workspace_type.return_value.inbox.return_value = []
            with self.assertRaisesRegex(RuntimeError, "controller_peer_mismatch"):
                bridge.main()


if __name__ == "__main__":
    unittest.main()
