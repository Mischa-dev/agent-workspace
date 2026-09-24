#!/usr/bin/env python3
"""Answer one Agent Workspace reply request at a time through an AgentOS agent.

Runs once per timer activation or wakeup. Receipts make a lost HTTP/SSH
response safe to resume without submitting a replacement model task.

Agent Workspace side: MCP tools inbox_list, reply_claim, reply_send,
reply_pass, reply_release. See apps/server/src/mcp/replies.rs.
"""

import argparse
import fcntl
import json
import os
from pathlib import Path
import subprocess
import sys
import tempfile
import time
import uuid


STATE = Path.home() / ".local/share/agent-workspace-aw-bridge"
TERMINAL = {"completed", "failed", "interrupted", "cancelled", "canceled"}
RENEW_SECONDS = 240
TASK_DEADLINE_SECONDS = 3600
MAX_REPLY_CHARACTERS = 19500


def save(path, value):
    path.parent.mkdir(mode=0o700, parents=True, exist_ok=True)
    fd, temporary = tempfile.mkstemp(prefix=".receipt-", dir=path.parent)
    try:
        os.fchmod(fd, 0o600)
        with os.fdopen(fd, "w") as stream:
            json.dump(value, stream, indent=2)
            stream.write("\n")
            stream.flush()
            os.fsync(stream.fileno())
        os.replace(temporary, path)
    finally:
        if os.path.exists(temporary):
            os.unlink(temporary)


def controller(command):
    with tempfile.TemporaryDirectory(prefix="workspace-agentos-") as directory:
        request = Path(directory) / "request.json"
        save(request, {"protocol_version": 1, "context": "nyx-crew", "command": command})
        result = subprocess.run(
            ["agentos", "controller", "call", str(request)],
            capture_output=True, text=True, timeout=45, check=False,
        )
    try:
        envelope = json.loads(result.stdout)
    except ValueError as error:
        raise RuntimeError("AgentOS response was unreadable; preserve the receipt") from error
    if result.returncode or envelope.get("status") != "ok":
        issue = envelope.get("issue") or {}
        raise RuntimeError(f"AgentOS rejected {command['operation']}: {issue.get('code', 'unknown')}")
    return envelope["result"]


class Workspace:
    """A minimal MCP client over curl.

    Cloudflare rejects Python urllib's client signature, so requests go through
    curl. The bearer is passed on stdin, never as a process argument.
    """

    def __init__(self, config):
        self.endpoint = config["origin"].rstrip("/") + "/api/v1/mcp"
        self.token = Path(config["token_file"]).read_text().strip()
        self.session = None
        self.next_id = 1

    def _post(self, payload):
        with tempfile.TemporaryDirectory(prefix="workspace-mcp-") as directory:
            body = Path(directory) / "body.json"
            headers = Path(directory) / "headers.txt"
            save(body, payload)
            settings = [
                f'url = "{self.endpoint}"',
                f'header = "Authorization: Bearer {self.token}"',
                'header = "Content-Type: application/json"',
                'header = "Accept: application/json, text/event-stream"',
                f'data-binary = "@{body}"',
                f'dump-header = "{headers}"',
            ]
            if self.session:
                settings += [
                    f'header = "Mcp-Session-Id: {self.session}"',
                    'header = "Mcp-Protocol-Version: 2025-06-18"',
                ]
            result = subprocess.run(
                ["curl", "--silent", "--show-error", "--fail-with-body",
                 "--max-time", "30", "--config", "-"],
                input="\n".join(settings) + "\n", capture_output=True, text=True,
                timeout=35, check=False,
            )
            header_text = headers.read_text() if headers.exists() else ""
        if result.returncode:
            raise RuntimeError(f"Workspace MCP request failed: curl exit {result.returncode}")
        return header_text, result.stdout

    def _rpc(self, method, params=None, notification=False):
        payload = {"jsonrpc": "2.0", "method": method}
        if params is not None:
            payload["params"] = params
        if not notification:
            payload["id"] = self.next_id
            self.next_id += 1
        headers, text = self._post(payload)
        if notification:
            return headers, None
        # Streamable HTTP may answer as server-sent events; the first event
        # can be an empty priming message.
        lines = [
            line[6:] for line in text.splitlines()
            if line.startswith("data: ") and line[6:].strip()
        ]
        try:
            message = json.loads(lines[0] if lines else text)
        except ValueError as error:
            raise RuntimeError(f"Workspace returned unreadable {method} result") from error
        if "error" in message:
            raise RuntimeError(f"Workspace rejected {method}: {message['error'].get('message')}")
        return headers, message["result"]

    def connect(self):
        headers, _ = self._rpc("initialize", {
            "protocolVersion": "2025-06-18",
            "capabilities": {},
            "clientInfo": {"name": "agentos-nyx-bridge", "version": "2"},
        })
        for line in headers.splitlines():
            name, _, value = line.partition(":")
            if name.strip().lower() == "mcp-session-id":
                self.session = value.strip()
        if not self.session:
            raise RuntimeError("Workspace did not open an MCP session")
        self._rpc("notifications/initialized", notification=True)

    def tool(self, name, arguments):
        _, result = self._rpc("tools/call", {"name": name, "arguments": arguments})
        value = result.get("structuredContent") or {}
        outcome = value.get("outcome")
        if outcome == "ok":
            return value["value"]
        if outcome == "conflict":
            raise Conflict(value.get("message", "request is no longer usable"))
        raise RuntimeError(f"Workspace {name} failed: {outcome}")

    def inbox(self):
        return self.tool("inbox_list", {"limit": 50})["requests"]


class Conflict(Exception):
    """The request moved on (superseded, expired, answered). Not an error."""


def prompt_for(claim, name):
    request = claim["request"]
    lines = [
        f"You are {name}, participating in Mischa's private agent workspace.",
        claim["instructions"],
    ]
    if request["required"]:
        lines.append("Return the text that should appear as your workspace reply.")
    else:
        lines.append(
            "Return exactly JSON: {\"decision\":\"pass\"} if you have nothing useful to add, "
            "or {\"decision\":\"reply\",\"body\":\"your message\"} if you do."
        )
    lines += [
        "Use your existing AgentOS role and tools. Do not claim that you posted the reply yourself. "
        "Do not make unrelated external changes.",
        f"Conversation: {request['conversationName']} (with {request['otherMembers']})",
        "Recent conversation (oldest first):\n" + "\n".join(
            f"{'>> ' if m['messageId'] == request['messageId'] else ''}{m['author']}: {m['text']}"
            for m in claim["recentMessages"]
        ),
        "The message marked >> is the one you were asked about.",
    ]
    return "\n\n".join(lines)


def output_text(observation):
    output = observation.get("output")
    if isinstance(output, str):
        answer = output.strip()
    elif output is not None:
        answer = json.dumps(output, ensure_ascii=False)
    else:
        answer = ""
    if len(answer) > MAX_REPLY_CHARACTERS:
        answer = answer[:MAX_REPLY_CHARACTERS] + "\n\n[Truncated by the Workspace bridge.]"
    return answer


def participation(output):
    """Group requests may pass. Accept JSON decisions or plain reply text."""
    if not output or output.strip().lower().rstrip(".") == "pass":
        return "pass", ""
    content = output.strip()
    if content.startswith("```") and content.endswith("```"):
        content = content.split("\n", 1)[-1].rsplit("```", 1)[0].strip()
    try:
        value = json.loads(content)
    except ValueError:
        return "reply", output
    if isinstance(value, dict) and value.get("decision") == "pass":
        return "pass", ""
    if isinstance(value, dict) and value.get("decision") == "reply":
        body = value.get("body")
        if isinstance(body, str) and body.strip():
            return "reply", body.strip()
    raise RuntimeError("AgentOS returned an invalid group participation decision")


def process(workspace, config, receipt_path, receipt):
    request_id = receipt["request_id"]
    try:
        claim = workspace.tool("reply_claim", {"requestId": request_id})
    except Conflict as conflict:
        print(f"{config['slug']}: request {request_id} moved on ({conflict})")
        receipt["workspace_completed"] = True
        save(receipt_path, receipt)
        return
    generation = claim["request"]["generation"]
    required = claim["request"]["required"]
    if receipt.get("generation") not in (None, generation):
        # A person retried this request: start a fresh model task.
        for key in ("submission", "task"):
            receipt.pop(key, None)
    receipt["generation"] = generation
    save(receipt_path, receipt)
    last_renew = time.monotonic()
    if "submission" not in receipt:
        plan = controller({"operation": "plan"})
        receipt["submission"] = {
            "operation": "task-submit",
            "idempotency_key": f"workspace-{request_id}-{generation}",
            "submission": {
                "agent": config["agentos_id"],
                "source_revision": plan["source_revision"],
                "prompt": prompt_for(claim, config["slug"].capitalize()),
            },
        }
        save(receipt_path, receipt)
    if "task" not in receipt:
        receipt["task"] = controller(receipt["submission"])
        save(receipt_path, receipt)
    if not receipt["task"].get("runtime_run_id"):
        raise RuntimeError("AgentOS acceptance is uncertain; preserve the receipt")
    deadline = time.monotonic() + TASK_DEADLINE_SECONDS
    while time.monotonic() < deadline:
        if time.monotonic() - last_renew >= RENEW_SECONDS:
            try:
                workspace.tool("reply_claim", {"requestId": request_id})
            except Conflict as conflict:
                print(f"{config['slug']}: claim lost while working ({conflict})")
                receipt["workspace_completed"] = True
                save(receipt_path, receipt)
                return
            last_renew = time.monotonic()
        current = controller({"operation": "task-inspect", "id": receipt["task"]["id"], "refresh": True})["task"]
        receipt["task"] = current
        save(receipt_path, receipt)
        observation = current.get("observation") or {}
        status = observation.get("status", "waiting")
        if status not in TERMINAL:
            time.sleep(8)
            continue
        base = {"requestId": request_id, "generation": generation}
        try:
            if status != "completed":
                workspace.tool("reply_release", {**base, "reason": f"My AgentOS task ended as {status}."})
                outcome = "released"
            else:
                text = output_text(observation)
                action, body = ("reply", text) if required else participation(text)
                if action == "pass" or not body:
                    if required:
                        workspace.tool("reply_release", {**base, "reason": "My task finished without an answer."})
                        outcome = "released"
                    else:
                        workspace.tool("reply_pass", base)
                        outcome = "passed"
                else:
                    workspace.tool("reply_send", {**base, "text": body})
                    outcome = "replied"
        except Conflict as conflict:
            outcome = f"skipped ({conflict})"
        receipt["workspace_completed"] = True
        save(receipt_path, receipt)
        print(f"{config['slug']}: {outcome} {request_id}")
        return
    print(f"{config['slug']}: task still running; receipt retained")


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--agent", required=True)
    parser.add_argument("--check", action="store_true", help="Verify the connection and inbox only")
    args = parser.parse_args()
    config = json.loads((STATE / "agents.json").read_text())[args.agent]
    config["slug"] = args.agent
    receipts = STATE / "receipts" / args.agent
    receipts.mkdir(mode=0o700, parents=True, exist_ok=True)
    lock_path = STATE / (args.agent + ".lock")
    with lock_path.open("a+") as lock:
        fcntl.flock(lock, fcntl.LOCK_EX | fcntl.LOCK_NB)
        workspace = Workspace(config)
        workspace.connect()
        inbox = workspace.inbox()
        print(f"{args.agent}: connected; {len(inbox)} open request(s)")
        if args.check:
            return
        # Finish anything this agent already started before taking new work.
        open_ids = {item["requestId"] for item in inbox}
        for path in sorted(receipts.glob("*.json")):
            receipt = json.loads(path.read_text())
            if receipt.get("workspace_completed"):
                continue
            if receipt["request_id"] in open_ids:
                process(workspace, config, path, receipt)
                return
            receipt["workspace_completed"] = True
            save(path, receipt)
        for item in inbox:
            path = receipts / (item["requestId"] + ".json")
            receipt = {"request_id": item["requestId"], "started": str(uuid.uuid4())}
            save(path, receipt)
            process(workspace, config, path, receipt)
            return


if __name__ == "__main__":
    try:
        main()
    except BlockingIOError:
        print("Another run for this agent is already in progress")
    except Exception as error:
        print(f"Bridge stopped: {error}", file=sys.stderr)
        sys.exit(1)
