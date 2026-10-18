"""The runner side of Agent Workspace's agent runner contract.

See docs/agent-runner.md: verify a wakeup, claim the request, bind your
session, report progress, post interim and final replies, and stop when a
person presses Stop. Standard library only; Python 3.10 or later.
"""

from __future__ import annotations

import hashlib
import hmac
import json
import os
import re
import sys
import threading
import time
import urllib.error
import urllib.request
from collections import OrderedDict
from dataclasses import dataclass, field
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
from typing import Any, Callable, Mapping

__all__ = [
    "WAKEUP_TOLERANCE_SECONDS",
    "WakeupError",
    "sign_wakeup",
    "verify_wakeup",
    "DeliveryDeduper",
    "McpError",
    "ToolError",
    "McpClient",
    "HttpResponse",
    "ReplyTools",
    "parse_settings",
    "strip_leading_mentions",
    "RequestContext",
    "Runner",
    "runner_settings_from_env",
]

# ---------------------------------------------------------------------------
# Wakeups

#: Five minutes, as docs/agent-runner.md asks.
WAKEUP_TOLERANCE_SECONDS = 300


class WakeupError(Exception):
    def __init__(self, message: str, status: int = 401) -> None:
        super().__init__(message)
        self.status = status


def _header(headers: Mapping[str, Any], name: str) -> str | None:
    getter = getattr(headers, "get", None)
    value = getter(name) if getter else None
    if value is None:
        for key, candidate in headers.items():
            if key.lower() == name:
                value = candidate
                break
    if isinstance(value, (list, tuple)):
        value = value[0] if value else None
    return value


def sign_wakeup(secret: str, timestamp: str, body: str | bytes) -> str:
    """The signature on a wakeup: ``sha256=`` and the hex HMAC of ``"{timestamp}.{body}"``."""
    raw = body.encode() if isinstance(body, str) else body
    digest = hmac.new(secret.encode(), timestamp.encode() + b"." + raw, hashlib.sha256).hexdigest()
    return f"sha256={digest}"


def verify_wakeup(
    secret: str,
    headers: Mapping[str, Any],
    body: str | bytes,
    now: float | None = None,
    tolerance_seconds: int = WAKEUP_TOLERANCE_SECONDS,
) -> dict[str, Any]:
    """Checks a wakeup's signature (in constant time) and age, and parses it.

    Raises ``WakeupError`` for anything that isn't a genuine, fresh wakeup.
    """
    timestamp = _header(headers, "x-workspace-timestamp")
    signature = _header(headers, "x-workspace-signature")
    if not timestamp or not signature or not re.fullmatch(r"\d{1,12}", timestamp):
        raise WakeupError("missing signature")
    current = time.time() if now is None else now
    if abs(current - int(timestamp)) > tolerance_seconds:
        raise WakeupError("stale wakeup")
    if not hmac.compare_digest(sign_wakeup(secret, timestamp, body).encode(), signature.encode()):
        raise WakeupError("bad signature")
    try:
        wakeup = json.loads(body)
    except ValueError:
        raise WakeupError("body is not JSON", 400) from None
    if not isinstance(wakeup, dict) or not isinstance(wakeup.get("event"), str) or not isinstance(wakeup.get("deliveryId"), str):
        raise WakeupError("not a wakeup", 400)
    delivery = _header(headers, "x-workspace-delivery")
    if delivery and delivery != wakeup["deliveryId"]:
        raise WakeupError("delivery mismatch", 400)
    return wakeup


class DeliveryDeduper:
    """Remembers recent delivery IDs, so a retried wakeup is handled once."""

    def __init__(self, limit: int = 10_000) -> None:
        self._seen: OrderedDict[str, None] = OrderedDict()
        self._limit = limit
        self._lock = threading.Lock()

    def first(self, delivery_id: str) -> bool:
        """True the first time an ID is seen."""
        with self._lock:
            if delivery_id in self._seen:
                return False
            self._seen[delivery_id] = None
            if len(self._seen) > self._limit:
                self._seen.popitem(last=False)
            return True


# ---------------------------------------------------------------------------
# MCP over streamable HTTP

PROTOCOL_VERSION = "2025-06-18"


class McpError(Exception):
    def __init__(self, message: str, code: int | None = None) -> None:
        super().__init__(message)
        self.code = code


class ToolError(Exception):
    """A reply tool answered with something other than ``ok``."""

    def __init__(self, outcome: str, detail: Mapping[str, Any] | None = None) -> None:
        detail = detail or {}
        message = detail.get("message") or (f"invalid {detail['field']}" if detail.get("field") else outcome)
        super().__init__(message)
        self.outcome = outcome
        self.field = detail.get("field")

    @property
    def retryable(self) -> bool:
        """``temporarily_unavailable``: worth retrying with backoff."""
        return self.outcome == "temporarily_unavailable"


@dataclass
class HttpResponse:
    status: int
    headers: Mapping[str, str]
    body: bytes


Transport = Callable[[str, Mapping[str, str], bytes], HttpResponse]


def _urllib_transport(url: str, headers: Mapping[str, str], body: bytes) -> HttpResponse:
    request = urllib.request.Request(url, data=body, headers=dict(headers), method="POST")
    try:
        with urllib.request.urlopen(request, timeout=60) as response:
            return HttpResponse(response.status, {k.lower(): v for k, v in response.headers.items()}, response.read())
    except urllib.error.HTTPError as error:
        return HttpResponse(error.code, {k.lower(): v for k, v in error.headers.items()}, error.read())


def _parse_sse(text: str) -> list[dict[str, Any]]:
    messages = []
    for block in re.split(r"\r?\n\r?\n", text):
        data = "\n".join(line[5:].lstrip() for line in re.split(r"\r?\n", block) if line.startswith("data:"))
        if data:
            messages.append(json.loads(data))
    return messages


def _tool_payload(result: Any) -> Any:
    if not isinstance(result, dict):
        return None
    if "structuredContent" in result:
        return result["structuredContent"]
    for part in result.get("content") or []:
        if part.get("type") == "text":
            try:
                return json.loads(part.get("text", ""))
            except ValueError:
                return part.get("text")
    return None


class McpClient:
    """A small MCP client: ``initialize``, then ``tools/call``. JSON and SSE answers both work."""

    def __init__(self, url: str, token: str, transport: Transport | None = None) -> None:
        self._url = url
        self._token = token
        self._transport = transport or _urllib_transport
        self._session: str | None = None
        self._ready = False
        self._next_id = 1
        self._lock = threading.Lock()

    def _post(self, message: dict[str, Any]) -> dict[str, Any] | None:
        headers = {
            "authorization": f"Bearer {self._token}",
            "content-type": "application/json",
            "accept": "application/json, text/event-stream",
        }
        if self._session:
            headers["mcp-session-id"] = self._session
            headers["mcp-protocol-version"] = PROTOCOL_VERSION
        response = self._transport(self._url, headers, json.dumps(message).encode())
        lowered = {k.lower(): v for k, v in response.headers.items()}
        if lowered.get("mcp-session-id"):
            self._session = lowered["mcp-session-id"]
        if response.status == 404 and self._session and message.get("method") != "initialize":
            # The server forgot the session (a restart): start over once.
            self._session = None
            self._ready = False
            raise McpError("session expired", 404)
        if not 200 <= response.status < 300:
            raise McpError(f"MCP endpoint answered {response.status}", response.status)
        if "id" not in message:
            return None
        text = response.body.decode()
        if "text/event-stream" in lowered.get("content-type", ""):
            messages = _parse_sse(text)
        else:
            messages = [json.loads(text)]
        answer = next((m for m in messages if m.get("id") == message["id"]), None)
        if answer is None:
            raise McpError("no answer to the request")
        if answer.get("error"):
            raise McpError(answer["error"].get("message", "error"), answer["error"].get("code"))
        return answer

    def _id(self) -> int:
        value = self._next_id
        self._next_id += 1
        return value

    def _initialize(self) -> None:
        if self._ready:
            return
        self._post(
            {
                "jsonrpc": "2.0",
                "id": self._id(),
                "method": "initialize",
                "params": {
                    "protocolVersion": PROTOCOL_VERSION,
                    "capabilities": {},
                    "clientInfo": {"name": "agent-workspace-runner", "version": "0.1.0"},
                },
            }
        )
        self._post({"jsonrpc": "2.0", "method": "notifications/initialized"})
        self._ready = True

    def call_tool(self, name: str, arguments: Mapping[str, Any]) -> Any:
        """Calls a tool and returns its structured result."""
        with self._lock:
            for attempt in range(2):
                try:
                    self._initialize()
                    answer = self._post(
                        {
                            "jsonrpc": "2.0",
                            "id": self._id(),
                            "method": "tools/call",
                            "params": {"name": name, "arguments": dict(arguments)},
                        }
                    )
                    return _tool_payload((answer or {}).get("result"))
                except McpError as error:
                    if attempt == 0 and error.code == 404:
                        continue
                    raise
        return None


# ---------------------------------------------------------------------------
# The reply tools


def _unwrap(payload: Any) -> Any:
    if isinstance(payload, dict) and payload.get("outcome") == "ok":
        return payload.get("value")
    outcome = payload.get("outcome", "invalid_response") if isinstance(payload, dict) else "invalid_response"
    raise ToolError(outcome, payload if isinstance(payload, dict) else None)


class ReplyTools:
    """The agent's reply tools, by their MCP names and arguments."""

    def __init__(self, mcp: McpClient) -> None:
        self.mcp = mcp

    def inbox_list(self, limit: int | None = None) -> list[dict[str, Any]]:
        return _unwrap(self.mcp.call_tool("inbox_list", {"limit": limit} if limit else {}))["requests"]

    def claim(self, request_id: str) -> dict[str, Any]:
        return _unwrap(self.mcp.call_tool("reply_claim", {"requestId": request_id}))

    def session_bind(
        self,
        conversation_id: str,
        external_ref: str,
        harness: str,
        url: str | None = None,
        state: str | None = None,
    ) -> dict[str, Any]:
        args: dict[str, Any] = {"conversationId": conversation_id, "externalRef": external_ref, "harness": harness}
        if url:
            args["url"] = url
        if state:
            args["state"] = state
        return _unwrap(self.mcp.call_tool("session_bind", args))

    def progress(self, request_id: str, generation: int, status: str) -> dict[str, Any]:
        return _unwrap(
            self.mcp.call_tool("reply_progress", {"requestId": request_id, "generation": generation, "status": status[:200]})
        )

    def send(
        self,
        request_id: str,
        generation: int,
        text: str,
        final: bool | None = None,
        client_request_id: str | None = None,
    ) -> dict[str, Any]:
        """``final=False`` posts an interim message and keeps the claim."""
        args: dict[str, Any] = {"requestId": request_id, "generation": generation, "text": text}
        if final is not None:
            args["final"] = final
        if client_request_id:
            args["clientRequestId"] = client_request_id
        return _unwrap(self.mcp.call_tool("reply_send", args))

    def pass_(self, request_id: str, generation: int | None = None, reason: str | None = None) -> dict[str, Any]:
        args: dict[str, Any] = {"requestId": request_id}
        if generation is not None:
            args["generation"] = generation
        if reason:
            args["reason"] = reason
        return _unwrap(self.mcp.call_tool("reply_pass", args))

    def release(self, request_id: str, reason: str, generation: int | None = None) -> dict[str, Any]:
        args: dict[str, Any] = {"requestId": request_id, "reason": reason}
        if generation is not None:
            args["generation"] = generation
        return _unwrap(self.mcp.call_tool("reply_release", args))


# ---------------------------------------------------------------------------
# Launch settings

_SETTINGS_LINE = re.compile(r"^Settings:\s*(.*)$")
_SETTING_KEY = re.compile(r"^[a-zA-Z][\w-]{0,40}$")


def parse_settings(text: str) -> tuple[str, dict[str, str]]:
    """Splits the ``Settings: model=<id> reasoning=<level> speed=<id>`` line
    a launch from the Launch dialog puts last in its message from the prompt."""
    lines = text.rstrip().split("\n")
    match = _SETTINGS_LINE.match(lines[-1].strip()) if lines else None
    if not match:
        return text.strip(), {}
    settings: dict[str, str] = {}
    for pair in match.group(1).split():
        key, _, value = pair.partition("=")
        if _SETTING_KEY.match(key) and value:
            settings[key] = value
    return "\n".join(lines[:-1]).strip(), settings


def strip_leading_mentions(text: str) -> str:
    """The message text without the @mentions it starts with."""
    rest = text.strip()
    while rest.startswith("@"):
        parts = re.split(r"\s", rest, maxsplit=1)
        rest = parts[1].lstrip(" \t\r\n,:") if len(parts) > 1 else ""
    return rest.strip()


# ---------------------------------------------------------------------------
# The runner


@dataclass
class RequestContext:
    """One claimed request, and the calls that answer it."""

    claim: dict[str, Any]
    request: dict[str, Any]
    #: The message that asked, without leading mentions or the settings line.
    prompt: str
    #: ``model``, ``reasoning``, ``speed`` from a final ``Settings:`` line.
    settings: dict[str, str]
    #: The session already bound to this conversation, or None.
    session_ref: str | None
    #: Set when a person presses Stop. Kill your child process on it.
    stopped: threading.Event
    _tools: ReplyTools = field(repr=False)
    _harness: str = field(repr=False)
    _listeners: list[Callable[["RequestContext"], None]] = field(default_factory=list, repr=False)
    _queued: list["RequestContext"] = field(default_factory=list, repr=False)
    _lock: threading.Lock = field(default_factory=threading.Lock, repr=False)

    @property
    def request_id(self) -> str:
        return self.request["requestId"]

    @property
    def generation(self) -> int:
        return self.request["generation"]

    def progress(self, status: str) -> None:
        """One status line under the agent's name. Renews the lease."""
        self._tools.progress(self.request_id, self.generation, status)

    def interim(self, text: str) -> None:
        """A message that keeps the claim."""
        self._tools.send(self.request_id, self.generation, text, final=False)

    def reply(self, text: str) -> None:
        """The final answer. Closes the request."""
        self._tools.send(self.request_id, self.generation, text)

    def pass_(self, reason: str | None = None) -> None:
        self._tools.pass_(self.request_id, self.generation, reason)

    def release(self, reason: str) -> None:
        self._tools.release(self.request_id, reason, self.generation)

    def bind_session(self, external_ref: str, url: str | None = None, state: str | None = None) -> None:
        self._tools.session_bind(self.request["replyConversationId"], external_ref, self._harness, url, state)

    def on_followup(self, listener: Callable[["RequestContext"], None]) -> None:
        """Follow-ups people send in the thread while this request runs."""
        with self._lock:
            self._listeners.append(listener)
            queued, self._queued = self._queued, []
        for followup in queued:
            listener(followup)

    def _deliver(self, followup: "RequestContext") -> None:
        with self._lock:
            listeners = list(self._listeners)
            if not listeners:
                self._queued.append(followup)
        for listener in listeners:
            listener(followup)


class Runner:
    def __init__(
        self,
        mcp_url: str,
        token: str,
        harness: str,
        on_request: Callable[[RequestContext], None],
        webhook_secret: str | None = None,
        transport: Transport | None = None,
        on_error: Callable[[BaseException], None] | None = None,
    ) -> None:
        self.tools = ReplyTools(McpClient(mcp_url, token, transport))
        self._harness = harness
        self._on_request = on_request
        self._webhook_secret = webhook_secret
        self._on_error = on_error
        self._deduper = DeliveryDeduper()
        self._active: dict[str, RequestContext] = {}
        self._handed_off: set[str] = set()
        self._lock = threading.Lock()

    def receive(self, headers: Mapping[str, Any], body: str | bytes) -> int:
        """Verifies one wakeup and handles it on a thread. Answer with the returned status right away."""
        if not self._webhook_secret:
            return 500
        try:
            wakeup = verify_wakeup(self._webhook_secret, headers, body)
        except WakeupError as error:
            return error.status
        if wakeup["event"] == "ping":
            # A test wakeup from the Agents page: answering is the whole test.
            return 204
        if self._deduper.first(wakeup["deliveryId"]):
            threading.Thread(target=self._dispatch_safely, args=(wakeup,), daemon=True).start()
        return 204

    def _dispatch_safely(self, wakeup: Mapping[str, Any]) -> None:
        try:
            self.dispatch(wakeup)
        except Exception as error:  # noqa: BLE001
            self._report(error)

    def dispatch(self, wakeup: Mapping[str, Any]) -> None:
        """Handles a wakeup that is already verified (or a request found by polling). Blocks while it runs."""
        request_id = wakeup["replyRequestId"]
        if wakeup.get("event") == "reply.cancelled":
            with self._lock:
                active = self._active.get(request_id)
            if active:
                active.stopped.set()
            return
        with self._lock:
            if request_id in self._active or request_id in self._handed_off:
                return
        claim = self.tools.claim(request_id)
        context = self._context(claim, wakeup.get("sessionRef") or (claim.get("session") or {}).get("externalRef"))
        follows = claim["request"].get("followsRequestId") or wakeup.get("followsRequestId")
        with self._lock:
            parent = self._active.get(follows) if follows else None
            if parent:
                # A follow-up joins the work already running in its thread.
                self._handed_off.add(context.request_id)
                context.stopped = parent.stopped
            else:
                self._active[context.request_id] = context
        if parent:
            parent._deliver(context)
            return
        try:
            self._on_request(context)
        except Exception as error:  # noqa: BLE001
            if not context.stopped.is_set():
                try:
                    context.release("The runner hit an error and stopped.")
                except Exception:  # noqa: BLE001
                    pass
                self._report(error)
        finally:
            with self._lock:
                self._active.pop(context.request_id, None)

    def _context(self, claim: dict[str, Any], session_ref: str | None) -> RequestContext:
        request = claim["request"]
        asked = next(
            (m.get("text", "") for m in claim.get("recentMessages") or [] if m.get("messageId") == request.get("messageId")),
            request.get("messageExcerpt", ""),
        )
        prompt, settings = parse_settings(strip_leading_mentions(asked))
        return RequestContext(
            claim=claim,
            request=request,
            prompt=prompt,
            settings=settings,
            session_ref=session_ref,
            stopped=threading.Event(),
            _tools=self.tools,
            _harness=self._harness,
        )

    def listen(self, port: int, host: str = "127.0.0.1", path: str = "/wake") -> ThreadingHTTPServer:
        """Serves wakeups on ``POST {path}`` from a background thread. Call ``shutdown()`` to stop."""
        runner = self

        class Handler(BaseHTTPRequestHandler):
            def do_POST(self) -> None:  # noqa: N802
                if self.path.split("?")[0] != path:
                    self.send_response(404)
                    self.end_headers()
                    return
                length = int(self.headers.get("content-length") or 0)
                if length > 1 << 20:
                    self.send_response(413)
                    self.end_headers()
                    return
                body = self.rfile.read(length)
                self.send_response(runner.receive(self.headers, body))
                self.end_headers()

            def log_message(self, format: str, *args: Any) -> None:  # noqa: A002
                pass

        server = ThreadingHTTPServer((host, port), Handler)
        threading.Thread(target=server.serve_forever, daemon=True).start()
        return server

    def poll(self, interval_seconds: float = 30, stop: threading.Event | None = None) -> None:
        """Without a public URL: polls ``inbox_list`` until ``stop`` is set."""
        stop = stop or threading.Event()
        while not stop.is_set():
            try:
                for item in self.tools.inbox_list():
                    # Claimed ones too: after a restart, claiming again picks them up.
                    with self._lock:
                        busy = item["requestId"] in self._active
                    if item.get("status") not in ("pending", "claimed") or busy:
                        continue
                    event = "reply.followup" if item.get("followsRequestId") else "reply.requested"
                    threading.Thread(
                        target=self._dispatch_safely,
                        args=({"event": event, "replyRequestId": item["requestId"]},),
                        daemon=True,
                    ).start()
            except Exception as error:  # noqa: BLE001
                self._report(error)
            stop.wait(interval_seconds)

    def _report(self, error: BaseException) -> None:
        if self._on_error:
            self._on_error(error)
        else:
            print(f"agent runner: {error!r}", file=sys.stderr)


def runner_settings_from_env(env: Mapping[str, str] | None = None) -> dict[str, Any]:
    """Reads the runner settings Agent Workspace hands out (AGENT_WORKSPACE_*)."""
    env = os.environ if env is None else env

    def need(name: str) -> str:
        value = env.get(name)
        if not value:
            raise KeyError(f"{name} is not set")
        return value

    return {
        "mcp_url": need("AGENT_WORKSPACE_MCP_URL"),
        "token": need("AGENT_WORKSPACE_TOKEN"),
        "webhook_secret": env.get("AGENT_WORKSPACE_WEBHOOK_SECRET"),
        "harness": env.get("AGENT_WORKSPACE_RUNNER", "custom"),
    }
