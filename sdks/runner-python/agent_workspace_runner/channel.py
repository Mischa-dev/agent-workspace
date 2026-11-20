"""The reference channel adapter (A2 3.10).

Everything an agent's chat integration needs from Agent Workspace, on the
event log and the agent tools: receive events (a stream, falling back to
long-poll, or a signed push), send and reply, edit, delete, react, show it's
working, and fetch attached files. Standard library only; Python 3.10 or
later.
"""

from __future__ import annotations

import base64
import hashlib
import hmac
import json
import os
import re
import sys
import threading
import time
import urllib.error
import urllib.parse
import urllib.request
from collections import OrderedDict
from typing import Any, Callable, Iterable, Mapping, Protocol

__all__ = [
    "PUSH_TOLERANCE_SECONDS",
    "PushError",
    "ToolError",
    "ChannelRefused",
    "sign_standard",
    "verify_push",
    "event_thing",
    "reply_request_id",
    "Deduper",
    "FileCursor",
    "WorkspaceChannel",
    "parse_sse_block",
]

#: Five minutes either way, as the delivery contract asks.
PUSH_TOLERANCE_SECONDS = 300


class PushError(Exception):
    def __init__(self, message: str, status: int = 401) -> None:
        super().__init__(message)
        self.status = status


class ToolError(Exception):
    """A tool answered with something other than ``ok``."""

    def __init__(self, outcome: str, detail: Mapping[str, Any] | None = None) -> None:
        detail = detail or {}
        super().__init__(detail.get("message") or (f"invalid {detail['field']}" if detail.get("field") else outcome))
        self.outcome = outcome
        self.field = detail.get("field")

    @property
    def retryable(self) -> bool:
        """``temporarily_unavailable``: worth retrying with backoff."""
        return self.outcome == "temporarily_unavailable"


class ChannelRefused(Exception):
    """The server refused the credential: reconnecting won't help."""

    def __init__(self, status: int, body: str = "") -> None:
        super().__init__(f"the server refused the credential ({status}) {body[:200]}".strip())
        self.status = status


def unwrap(payload: Any) -> Any:
    if isinstance(payload, dict) and payload.get("outcome") == "ok":
        return payload.get("value")
    detail = payload if isinstance(payload, dict) else {}
    raise ToolError(detail.get("outcome", "invalid_response"), detail)


# ---------------------------------------------------------------------------
# Events


def event_thing(event: Mapping[str, Any]) -> dict[str, str] | None:
    """What an event is about, such as ``{"kind": "reply_request", "id": ...}``."""
    thing = (event.get("data") or {}).get("thing")
    if isinstance(thing, dict) and isinstance(thing.get("kind"), str) and isinstance(thing.get("id"), str):
        return {"kind": thing["kind"], "id": thing["id"]}
    return None


def reply_request_id(event: Mapping[str, Any]) -> str | None:
    """``reply.*`` events: the reply request they are about."""
    thing = event_thing(event)
    if thing and thing["kind"] == "reply_request":
        return thing["id"]
    value = (event.get("data") or {}).get("replyRequestId")
    return value if isinstance(value, str) else None


def _matches(pattern: str, kind: str) -> bool:
    if pattern == "*":
        return True
    if pattern.endswith(".*"):
        return kind.startswith(pattern[:-1])
    return pattern == kind


# ---------------------------------------------------------------------------
# Pushes


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


def _bytes(body: str | bytes) -> bytes:
    return body.encode() if isinstance(body, str) else body


def sign_standard(secret: str, webhook_id: str, timestamp: str, body: str | bytes) -> str:
    """The Standard Webhooks signature: ``v1,`` and the base64 HMAC of ``{id}.{timestamp}.{body}``."""
    key = _key(secret[6:]) if secret.startswith("whsec_") else secret.encode()
    digest = hmac.new(key, f"{webhook_id}.{timestamp}.".encode() + _bytes(body), hashlib.sha256).digest()
    return "v1," + base64.b64encode(digest).decode()


def _key(encoded: str) -> bytes:
    """A ``whsec_`` secret's key: base64 in either alphabet, padded or not."""
    normal = encoded.replace("-", "+").replace("_", "/")
    return base64.b64decode(normal + "=" * (-len(normal) % 4))


def verify_push(
    secret: str | Iterable[str],
    headers: Mapping[str, Any],
    body: str | bytes,
    now: float | None = None,
    tolerance_seconds: int = PUSH_TOLERANCE_SECONDS,
) -> dict[str, Any]:
    """Checks a push (in constant time, against each secret) and its age, and returns its event.

    Raises ``PushError`` for anything that isn't a genuine, fresh push.
    """
    secrets = [secret] if isinstance(secret, str) else list(secret)
    webhook_id = _header(headers, "webhook-id")
    timestamp = _header(headers, "webhook-timestamp")
    signatures = _header(headers, "webhook-signature")
    if not webhook_id or not timestamp or not signatures:
        raise PushError("missing signature")
    current = time.time() if now is None else now
    if not re.fullmatch(r"\d{1,12}", timestamp) or abs(current - int(timestamp)) > tolerance_seconds:
        raise PushError("stale push")
    genuine = False
    for candidate in secrets:
        expected = sign_standard(candidate, webhook_id, timestamp, body).encode()
        genuine = genuine or any(hmac.compare_digest(given.encode(), expected) for given in signatures.split(" "))
    if not genuine:
        raise PushError("bad signature")
    try:
        parsed = json.loads(body)
    except ValueError:
        raise PushError("body is not JSON", 400) from None
    if not isinstance(parsed, dict) or not isinstance(parsed.get("type"), str):
        raise PushError("not an event", 400)
    event = parsed
    if webhook_id != event["id"]:
        raise PushError("delivery mismatch", 400)
    return event


class Deduper:
    """Remembers recent event ids, so a repeat is handled once."""

    def __init__(self, limit: int = 10_000) -> None:
        self._seen: OrderedDict[str, None] = OrderedDict()
        self._limit = limit
        self._lock = threading.Lock()

    def first(self, event_id: str) -> bool:
        """True the first time an id is seen."""
        with self._lock:
            if event_id in self._seen:
                return False
            self._seen[event_id] = None
            if len(self._seen) > self._limit:
                self._seen.popitem(last=False)
            return True


# ---------------------------------------------------------------------------
# Cursors


class CursorStore(Protocol):
    def load(self) -> str | None: ...

    def save(self, cursor: str) -> None: ...


class FileCursor:
    """A cursor kept in one file, replaced whole on each save."""

    def __init__(self, path: str) -> None:
        self.path = path

    def load(self) -> str | None:
        try:
            with open(self.path, encoding="utf-8") as file:
                return file.read().strip() or None
        except OSError:
            return None

    def save(self, cursor: str) -> None:
        with open(f"{self.path}.tmp", "w", encoding="utf-8") as file:
            file.write(cursor)
        os.replace(f"{self.path}.tmp", self.path)


# ---------------------------------------------------------------------------
# Reading


class Opened(Protocol):
    """What ``open_url`` returns: a status, headers and a body read by line."""

    status: int
    headers: Mapping[str, str]

    def readline(self) -> bytes: ...

    def read(self) -> bytes: ...

    def close(self) -> None: ...


Opener = Callable[[str, Mapping[str, str], float], Opened]


class _HttpErrorBody:
    def __init__(self, error: urllib.error.HTTPError) -> None:
        self.status = error.code
        self.headers = {k.lower(): v for k, v in error.headers.items()}
        self._error = error

    def readline(self) -> bytes:
        return self._error.readline()

    def read(self) -> bytes:
        return self._error.read()

    def close(self) -> None:
        self._error.close()


def _urllib_open(url: str, headers: Mapping[str, str], timeout: float) -> Opened:
    request = urllib.request.Request(url, headers=dict(headers), method="GET")
    try:
        response = urllib.request.urlopen(request, timeout=timeout)
    except urllib.error.HTTPError as error:
        return _HttpErrorBody(error)
    response.headers = {k.lower(): v for k, v in response.headers.items()}  # type: ignore[assignment]
    return response


def parse_sse_block(block: str) -> dict[str, Any] | None:
    """One SSE block as an event; comments and keep-alives give ``None``."""
    event_id = None
    data = []
    for line in re.split(r"\r?\n", block):
        if line.startswith(":"):
            continue
        field, _, value = line.partition(":")
        value = value[1:] if value.startswith(" ") else value
        if field == "id":
            event_id = value
        elif field == "data":
            data.append(value)
    if not data:
        return None
    event = json.loads("\n".join(data))
    if event.get("cursor") is None:
        event["cursor"] = event_id
    return event


Handler = Callable[[dict[str, Any]], Any]


class WorkspaceChannel:
    """One account's events and conversations, on one connection."""

    def __init__(
        self,
        origin: str,
        token: str,
        tools: Any,
        connection: str | None = None,
        cursor: CursorStore | None = None,
        poll: bool = False,
        opener: Opener | None = None,
        on_state: Callable[[str], None] | None = None,
        on_error: Callable[[BaseException], None] | None = None,
    ) -> None:
        self.origin = origin.rstrip("/")
        self.tools = tools
        self._token = token
        self._connection = connection
        self._store = cursor
        self._poll_only = poll
        self._open = opener or _urllib_open
        self._on_state = on_state
        self._on_error = on_error
        self._handlers: list[tuple[str, Handler]] = []
        self._deduper = Deduper()
        self._place = cursor.load() if cursor else None
        self._delivered = 0

    def on(self, pattern: str, handler: Handler) -> "WorkspaceChannel":
        """Handle events of a type, a family (``reply.*``) or every one (``*``)."""
        self._handlers.append((pattern, handler))
        return self

    @property
    def cursor(self) -> str | None:
        """The cursor after the last event handled."""
        return self._place

    def ack(self, cursor: str | None) -> None:
        """Remember that everything up to ``cursor`` is handled."""
        if not cursor:
            return
        self._place = cursor
        if self._store:
            self._store.save(cursor)

    def deliver(self, event: dict[str, Any]) -> None:
        """Hands one event to its handlers, once, then moves the cursor past it."""
        self._delivered += 1
        if self._deduper.first(event["id"]):
            for pattern, handler in self._handlers:
                if not _matches(pattern, event["type"]):
                    continue
                try:
                    handler(event)
                except Exception as error:  # noqa: BLE001
                    self._report(error)
        self.ack(event.get("cursor"))

    def receive_push(self, secret: str | Iterable[str], headers: Mapping[str, Any], body: str | bytes) -> int:
        """Checks a signed push and handles its event on a thread. Answer with the status right away."""
        try:
            event = verify_push(secret, headers, body)
        except PushError as error:
            return error.status
        if event["type"] != "ping":
            threading.Thread(target=self._deliver_safely, args=({**event, "cursor": None},), daemon=True).start()
        return 204

    def _deliver_safely(self, event: dict[str, Any]) -> None:
        try:
            self.deliver(event)
        except Exception as error:  # noqa: BLE001
            self._report(error)

    def connect(self, stop: threading.Event | None = None) -> None:
        """Reads events until ``stop`` is set: a stream, or long-polling where none can be had.

        A refused credential ends it with ``ChannelRefused``.
        """
        stop = stop or threading.Event()
        streaming = not self._poll_only
        failures = 0
        while not stop.is_set():
            self._state("connecting")
            started = time.monotonic()
            before = self._delivered
            try:
                if streaming:
                    streaming = self._stream(stop)
                else:
                    self._state("polling")
                    self.poll_once(wait=30)
                failures = 0
                # A server that answers at once every time is not waited on
                # in a tight loop.
                if self._delivered == before and time.monotonic() - started < 1:
                    stop.wait(1)
            except ChannelRefused:
                self._state("closed")
                raise
            except Exception as error:  # noqa: BLE001
                if stop.is_set():
                    break
                self._report(error)
                self._state("offline")
                failures += 1
                stop.wait(min(30, 2 ** min(failures, 5)))
        self._state("closed")

    def poll_once(self, wait: int = 0) -> list[dict[str, Any]]:
        """One page of events, waiting up to ``wait`` seconds for the first; handles them."""
        query = {"wait": str(wait)}
        if self._place:
            query["cursor"] = self._place
        response = self._open(self._url(query), self._headers("application/json"), wait + 30)
        try:
            self._check(response)
            page = json.loads(response.read() or b"{}")
        finally:
            response.close()
        self._handle_page(page)
        return page.get("events") or []

    def _handle_page(self, page: Mapping[str, Any]) -> None:
        for event in page.get("events") or []:
            self.deliver(event)
        self.ack(page.get("nextCursor"))

    def _stream(self, stop: threading.Event) -> bool:
        """Reads one stream to its end. False when the server doesn't stream here."""
        headers = self._headers("text/event-stream")
        if self._place:
            headers["last-event-id"] = self._place
        response = self._open(self._url({}), headers, 60)
        try:
            self._check(response)
            kind = response.headers.get("content-type", "")
            if "application/json" in kind:
                # Something on the way doesn't stream: this is a page, and
                # reading it may have moved the server's place.
                self._handle_page(json.loads(response.read() or b"{}"))
                return False
            if "text/event-stream" not in kind:
                return False
            self._state("streaming")
            lines: list[str] = []
            while not stop.is_set():
                raw = response.readline()
                if not raw:
                    break
                line = raw.decode("utf-8").rstrip("\r\n")
                if line:
                    lines.append(line)
                    continue
                event = parse_sse_block("\n".join(lines))
                lines = []
                if event:
                    self.deliver(event)
            return True
        finally:
            response.close()

    def _url(self, query: Mapping[str, str]) -> str:
        params = dict(query)
        if self._connection:
            params["connection"] = self._connection
        suffix = "?" + urllib.parse.urlencode(params) if params else ""
        return f"{self.origin}/api/v1/events{suffix}"

    def _headers(self, accept: str) -> dict[str, str]:
        return {"authorization": f"Bearer {self._token}", "accept": accept}

    def _check(self, response: Opened) -> None:
        if 200 <= response.status < 300:
            return
        text = response.read().decode("utf-8", "replace")
        if response.status in (401, 403):
            raise ChannelRefused(response.status, text)
        if self._place and response.status < 500 and "cursor" in text:
            # The cursor is past retention (``cursor_expired``) or not this
            # connection's: read on from where the server kept its place.
            self._place = None
        raise RuntimeError(f"events answered {response.status}: {text[:200]}")

    # -----------------------------------------------------------------------
    # Talking

    def send(
        self,
        conversation_id: str,
        text: str,
        reply: Mapping[str, Any] | None = None,
        client_message_id: str | None = None,
        reply_to_message_id: str | None = None,
        attachment_ids: list[str] | None = None,
    ) -> str | None:
        """Posts a message, or answers a reply request when ``reply`` (requestId, generation, final) is given."""
        if reply:
            args: dict[str, Any] = {"requestId": reply["requestId"], "generation": reply["generation"], "text": text}
            if reply.get("final") is not None:
                args["final"] = reply["final"]
            if client_message_id:
                args["clientRequestId"] = client_message_id
            if attachment_ids:
                args["attachmentIds"] = attachment_ids
            return (unwrap(self.tools.call_tool("reply_send", args)) or {}).get("messageId")
        args = {"conversationId": conversation_id, "text": text}
        if client_message_id:
            args["clientRequestId"] = client_message_id
        if reply_to_message_id:
            args["replyToMessageId"] = reply_to_message_id
        if attachment_ids:
            args["attachmentIds"] = attachment_ids
        value = unwrap(self.tools.call_tool("message_send", args)) or {}
        return value.get("messageId") or (value.get("message") or {}).get("id")

    def edit(self, message_id: str, text: str, client_request_id: str | None = None) -> None:
        args = {"messageId": message_id, "text": text}
        if client_request_id:
            args["clientRequestId"] = client_request_id
        unwrap(self.tools.call_tool("message_edit", args))

    def delete(self, message_id: str) -> None:
        unwrap(self.tools.call_tool("message_delete", {"messageId": message_id}))

    def react(self, message_id: str, emoji: str, reacted: bool = True) -> None:
        unwrap(self.tools.call_tool("message_react", {"messageId": message_id, "emoji": emoji, "reacted": reacted}))

    def typing(self, request_id: str, generation: int, status: str = "Working") -> None:
        """Shows what you're doing on a reply request you hold (and keeps it held)."""
        unwrap(self.tools.call_tool("reply_progress", {"requestId": request_id, "generation": generation, "status": status[:200]}))

    def download(self, file_id: str) -> tuple[str, str, bytes]:
        """Fetches a file attached to a message or task: ``(name, content type, bytes)``."""
        link = unwrap(self.tools.call_tool("file_link_get", {"fileId": file_id}))
        response = self._open(link["url"], {}, 60)
        try:
            if not 200 <= response.status < 300:
                raise RuntimeError(f"the file answered {response.status}")
            return link["name"], link["contentType"], response.read()
        finally:
            response.close()

    def upload(self, conversation_id: str, name: str, data: bytes, content_type: str | None = None) -> str:
        """Uploads a file to a conversation; pass its id in ``attachment_ids``."""
        args: dict[str, Any] = {"conversationId": conversation_id, "name": name, "dataBase64": base64.b64encode(data).decode()}
        if content_type:
            args["contentType"] = content_type
        value = unwrap(self.tools.call_tool("file_upload", args)) or {}
        file_id = value.get("fileId") or value.get("id") or (value.get("file") or {}).get("id")
        if not file_id:
            raise ToolError("invalid_response", {"message": "file_upload returned no id"})
        return file_id

    def _state(self, state: str) -> None:
        if self._on_state:
            self._on_state(state)

    def _report(self, error: BaseException) -> None:
        if self._on_error:
            self._on_error(error)
        else:
            print(f"agent workspace channel: {error!r}", file=sys.stderr)
