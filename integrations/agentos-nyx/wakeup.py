#!/usr/bin/env python3
"""Receive signed Agent Workspace pushes and start the matching bridge run.

A push only says "check your inbox"; the bridge still claims work through
MCP, so a missed or duplicate push is harmless. Pushes are Standard Webhooks
(`webhook-id`, `webhook-timestamp`, `webhook-signature`); only reply events
wake the bridge.

Listens on the address in agents.json ("wakeup_listen", e.g. the Tailscale IP
and a port). Each agent entry needs "workspace_agent_id" and
"webhook_secret_file" (the connection's `whsec_` secret).
"""

import base64
import hashlib
import hmac
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
import json
from pathlib import Path
import subprocess
import time

STATE = Path.home() / ".local/share/agent-workspace-aw-bridge"
MAX_BODY_BYTES = 65536
MAX_SKEW_SECONDS = 300


def load_agents():
    agents = json.loads((STATE / "agents.json").read_text())
    return {
        config["workspace_agent_id"]: (slug, Path(config["webhook_secret_file"]).read_text().strip())
        for slug, config in agents.items()
        if isinstance(config, dict) and config.get("webhook_secret_file")
    }


def standard_key(secret):
    """A `whsec_` secret's key: base64 in either alphabet, padded or not."""
    encoded = secret[6:] if secret.startswith("whsec_") else secret
    normal = encoded.replace("-", "+").replace("_", "/")
    try:
        key = base64.b64decode(normal + "=" * (-len(normal) % 4), validate=True)
    except ValueError:
        key = b""
    return key or encoded.encode()


def signed_by(secret, webhook_id, timestamp, signatures, body):
    """Whether any `v1,<base64>` signature is the secret's over id, timestamp and body."""
    digest = hmac.new(
        standard_key(secret), f"{webhook_id}.{timestamp}.".encode() + body, hashlib.sha256
    ).digest()
    expected = "v1," + base64.b64encode(digest).decode()
    return any(hmac.compare_digest(given, expected) for given in signatures.split(" "))


def wakes(event):
    """Reply events start a run; anything else (a test ping too) is answered and ignored."""
    return event.get("type", "").startswith("reply.")


class Handler(BaseHTTPRequestHandler):
    def do_POST(self):
        length = int(self.headers.get("Content-Length") or 0)
        if self.path != "/wake" or not 0 < length <= MAX_BODY_BYTES:
            return self.reply(404)
        body = self.rfile.read(length)
        try:
            event = json.loads(body)
            account = (event.get("recipient") or {}).get("accountId") or event.get("agentId")
            slug, secret = load_agents()[account]
        except (ValueError, KeyError, OSError, AttributeError):
            return self.reply(404)
        webhook_id = self.headers.get("webhook-id", "")
        timestamp = self.headers.get("webhook-timestamp", "")
        signatures = self.headers.get("webhook-signature", "")
        if not webhook_id or not signed_by(secret, webhook_id, timestamp, signatures, body):
            return self.reply(401)
        if not timestamp.isdigit() or abs(time.time() - int(timestamp)) > MAX_SKEW_SECONDS:
            return self.reply(401)
        if not wakes(event):
            return self.reply(204)
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
