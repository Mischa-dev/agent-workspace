#!/usr/bin/env python3
"""Receive signed Agent Workspace wakeups and start the matching bridge run.

A wakeup only says "check your inbox"; the bridge still claims work through
MCP, so a missed or duplicate wakeup is harmless.

Listens on the address in agents.json ("wakeup_listen", e.g. the Tailscale IP
and a port). Each agent entry needs "workspace_agent_id" and
"webhook_secret_file".
"""

import hashlib
import hmac
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
import json
from pathlib import Path
import subprocess
import time

STATE = Path.home() / ".local/share/agent-workspace-aw-bridge"
MAX_BODY_BYTES = 4096
MAX_SKEW_SECONDS = 300


def load_agents():
    agents = json.loads((STATE / "agents.json").read_text())
    return {
        config["workspace_agent_id"]: (slug, Path(config["webhook_secret_file"]).read_text().strip())
        for slug, config in agents.items()
        if isinstance(config, dict) and config.get("webhook_secret_file")
    }


class Handler(BaseHTTPRequestHandler):
    def do_POST(self):
        length = int(self.headers.get("Content-Length") or 0)
        if self.path != "/wake" or not 0 < length <= MAX_BODY_BYTES:
            return self.reply(404)
        body = self.rfile.read(length)
        try:
            event = json.loads(body)
            slug, secret = load_agents()[event["agentId"]]
        except (ValueError, KeyError, OSError):
            return self.reply(404)
        timestamp = self.headers.get("X-Workspace-Timestamp", "")
        signature = self.headers.get("X-Workspace-Signature", "")
        expected = "sha256=" + hmac.new(
            secret.encode(), timestamp.encode() + b"." + body, hashlib.sha256
        ).hexdigest()
        if not hmac.compare_digest(signature, expected):
            return self.reply(401)
        if not timestamp.isdigit() or abs(time.time() - int(timestamp)) > MAX_SKEW_SECONDS:
            return self.reply(401)
        # --no-block: answer the webhook now; the unit's lock prevents overlap
        # with a timer run that is already going.
        subprocess.run(
            ["systemctl", "--user", "start", "--no-block", f"aw-crew@{slug}.service"],
            check=False, timeout=10,
        )
        return self.reply(202)

    def reply(self, status):
        self.send_response(status)
        self.send_header("Content-Length", "0")
        self.end_headers()

    def log_message(self, format, *args):
        print(f"wakeup: {self.address_string()} {format % args}")


def main():
    config = json.loads((STATE / "agents.json").read_text())
    host, _, port = config["wakeup_listen"].rpartition(":")
    ThreadingHTTPServer((host, int(port)), Handler).serve_forever()


if __name__ == "__main__":
    main()
