import base64
import hashlib
import hmac
import io
import json
import os
import threading
import time
import unittest

import agent_workspace_runner as aw
from agent_workspace_runner import HttpResponse

SECRET = "whsec_" + base64.b64encode(b"test-webhook-secret").decode()
OTHER = "whsec_" + base64.b64encode(b"other").decode()
FIXTURES = os.path.join(os.path.dirname(__file__), "..", "..", "fixtures", "delivery")


def fixture(name):
    with open(os.path.join(FIXTURES, name), encoding="utf-8") as file:
        return file.read()


def event_body(kind, request_id, event_id, **data):
    """A ``reply.*`` event, as the server pushes it."""
    return json.dumps(
        {
            "type": kind,
            "timestamp": "2026-11-17T10:00:00Z",
            "id": event_id,
            "cursor": f"c-{event_id}",
            "reason": "reply_required",
            "data": {"thing": {"kind": "reply_request", "id": request_id}, "conversationId": "c", "generation": 3, **data},
        }
    )


def signed(body, timestamp=None, secret=SECRET):
    """Signed the Standard Webhooks way, as a push to a ``standard`` connection."""
    timestamp = str(int(time.time()) if timestamp is None else timestamp)
    event_id = json.loads(body)["id"]
    return {
        "webhook-id": event_id,
        "webhook-timestamp": timestamp,
        "webhook-signature": aw.sign_standard(secret, event_id, timestamp, body),
    }


class FakeMcp:
    """An MCP endpoint in memory: a session, then tool answers from `tools`."""

    def __init__(self, tools, sse=False):
        self.tools = tools
        self.sse = sse
        self.calls = []
        self.sessions = []

    def __call__(self, url, headers, body):
        message = json.loads(body)
        self.sessions.append(headers.get("mcp-session-id"))
        assert headers["authorization"] == "Bearer test-token"
        if message["method"] == "initialize":
            answer = {"jsonrpc": "2.0", "id": message["id"], "result": {"protocolVersion": "2025-06-18"}}
            return HttpResponse(200, {"content-type": "application/json", "mcp-session-id": "s1"}, json.dumps(answer).encode())
        if "id" not in message:
            return HttpResponse(202, {}, b"")
        name, args = message["params"]["name"], message["params"]["arguments"]
        self.calls.append((name, args))
        payload = self.tools(name, args)
        result = {"content": [{"type": "text", "text": json.dumps(payload)}], "structuredContent": payload}
        answer = json.dumps({"jsonrpc": "2.0", "id": message["id"], "result": result})
        if self.sse:
            return HttpResponse(200, {"content-type": "text/event-stream"}, f"event: message\ndata: {answer}\n\n".encode())
        return HttpResponse(200, {"content-type": "application/json"}, answer.encode())


def request(request_id, text, follows=None):
    item = {
        "requestId": request_id,
        "messageId": f"m-{request_id}",
        "messageExcerpt": text[:20],
        "generation": 3,
        "status": "claimed",
        "replyConversationId": "thread-1",
        "followsRequestId": follows,
    }
    return {
        "outcome": "ok",
        "value": {
            "request": item,
            "recentMessages": [{"messageId": f"m-{request_id}", "text": text}],
            "instructions": "",
            "session": None,
        },
    }


class PushTests(unittest.TestCase):
    def test_genuine_push_verifies(self):
        body = event_body("reply.requested", "r1", "evt_1")
        self.assertEqual(aw.reply_request_id(aw.verify_push(SECRET, signed(body), body)), "r1")
        self.assertEqual(aw.verify_push([OTHER, SECRET], signed(body), body)["id"], "evt_1")

    def test_tampered_stale_unsigned_or_wrong_secret_fail(self):
        body = event_body("reply.requested", "r1", "evt_1")
        with self.assertRaises(aw.PushError):
            aw.verify_push(SECRET, signed(body), body.replace("r1", "r2"))
        with self.assertRaises(aw.PushError):
            aw.verify_push(SECRET, signed(body, timestamp=int(time.time()) - 301), body)
        with self.assertRaises(aw.PushError):
            aw.verify_push(SECRET, {}, body)
        with self.assertRaises(aw.PushError):
            aw.verify_push(SECRET, signed(body, secret=OTHER), body)

    def test_the_published_fixtures_verify(self):
        standard = json.loads(fixture("webhook-standard.json"))
        now = int(standard["headers"]["webhook-timestamp"])
        event = aw.verify_push(standard["secret"], standard["headers"], standard["body"], now=now)
        self.assertEqual(event["type"], "message.mentioned")

    def test_a_server_secret_in_the_url_safe_alphabet_verifies(self):
        # The server makes secrets as unpadded URL-safe base64.
        secret = "whsec_" + base64.urlsafe_b64encode(bytes(range(250, 256)) * 5 + b"\xfb\xff").decode().rstrip("=")
        body = json.dumps({"type": "ping", "id": "evt_1"})
        timestamp = str(int(time.time()))
        headers = {
            "webhook-id": "evt_1",
            "webhook-timestamp": timestamp,
            "webhook-signature": aw.sign_standard(secret, "evt_1", timestamp, body),
        }
        self.assertEqual(aw.verify_push(secret, headers, body)["type"], "ping")
        key = bytes(range(250, 256)) * 5 + b"\xfb\xff"
        digest = hmac.new(key, f"evt_1.{timestamp}.{body}".encode(), hashlib.sha256).digest()
        self.assertEqual(headers["webhook-signature"], "v1," + base64.b64encode(digest).decode())

    def test_events_are_handled_once(self):
        deduper = aw.Deduper(limit=2)
        self.assertTrue(deduper.first("a"))
        self.assertFalse(deduper.first("a"))
        deduper.first("b")
        deduper.first("c")
        self.assertTrue(deduper.first("a"))


class SettingsTests(unittest.TestCase):
    def test_settings_come_off_the_last_line(self):
        prompt, settings = aw.parse_settings("Fix the bug\nSettings: model=m1 reasoning=high speed=fast")
        self.assertEqual(prompt, "Fix the bug")
        self.assertEqual(settings, {"model": "m1", "reasoning": "high", "speed": "fast"})
        self.assertEqual(aw.parse_settings("no settings here"), ("no settings here", {}))
        self.assertEqual(aw.strip_leading_mentions("@bot, @other: do it"), "do it")


class McpTests(unittest.TestCase):
    def test_json_and_sse_answers(self):
        for sse in (False, True):
            fake = FakeMcp(lambda name, args: {"outcome": "ok", "value": {"requests": [], "guidance": ""}}, sse=sse)
            client = aw.McpClient("https://example.test/api/v1/mcp", "test-token", transport=fake)
            tools = aw.ReplyTools(client)
            self.assertEqual(tools.inbox_list(), [])
            self.assertEqual(tools.inbox_list(5), [])
            self.assertEqual(fake.sessions, [None, "s1", "s1", "s1"])
            self.assertEqual(fake.calls[1], ("inbox_list", {"limit": 5}))

    def test_refusals_raise_tool_error(self):
        fake = FakeMcp(lambda name, args: {"outcome": "conflict", "message": "Stopped by Mischa"})
        tools = aw.ReplyTools(aw.McpClient("https://example.test/mcp", "test-token", transport=fake))
        with self.assertRaises(aw.ToolError) as caught:
            tools.send("r1", 0, "hi")
        self.assertEqual(caught.exception.outcome, "conflict")
        self.assertFalse(caught.exception.retryable)


class RunnerTests(unittest.TestCase):
    def test_claim_work_answer_stop_and_followups(self):
        claims = {"r1": request("r1", "@bot fix it\nSettings: model=m1"), "r2": request("r2", "also this", follows="r1")}

        def tools(name, args):
            if name == "reply_claim":
                return claims[args["requestId"]]
            if name == "reply_send" and args["requestId"] == "r1":
                return {"outcome": "conflict", "message": "stopped"}
            return {"outcome": "ok", "value": {"requestId": args.get("requestId"), "status": "ok"}}

        fake = FakeMcp(tools)
        seen = {}
        followup_seen = threading.Event()
        started = threading.Event()

        def on_request(context):
            seen["prompt"], seen["settings"] = context.prompt, context.settings
            context.bind_session("session-9")
            context.progress("Working")

            def on_followup(followup):
                seen["followup"] = followup.prompt
                followup.pass_()
                followup_seen.set()

            context.on_followup(on_followup)
            started.set()
            self.assertTrue(context.stopped.wait(5))
            try:
                context.reply("too late")
            except aw.ToolError as error:
                seen["late"] = error.outcome

        runner = aw.Runner("https://example.test/mcp", "test-token", "custom", on_request, webhook_secret=SECRET, transport=fake)

        def wake(event, request_id, delivery, **extra):
            body = event_body(event, request_id, delivery, **extra)
            return runner.receive(signed(body), body)

        self.assertEqual(wake("reply.requested", "r1", "d1"), 204)
        self.assertEqual(wake("reply.requested", "r1", "d1"), 204)  # a retry: ignored
        self.assertTrue(started.wait(5))
        wake("reply.followup", "r2", "d2", followsRequestId="r1")
        self.assertTrue(followup_seen.wait(5))
        wake("reply.cancelled", "r1", "d3")
        deadline = time.time() + 5
        while time.time() < deadline and "late" not in seen:
            time.sleep(0.01)

        self.assertEqual(seen["prompt"], "fix it")
        self.assertEqual(seen["settings"], {"model": "m1"})
        self.assertEqual(seen["followup"], "also this")
        self.assertEqual(seen["late"], "conflict")
        names = [name for name, _ in fake.calls]
        self.assertEqual(names.count("reply_claim"), 2)
        self.assertIn(("session_bind", {"conversationId": "thread-1", "externalRef": "session-9", "harness": "custom"}), fake.calls)
        self.assertIn(("reply_progress", {"requestId": "r1", "generation": 3, "status": "Working"}), fake.calls)
        self.assertIn(("reply_pass", {"requestId": "r2", "generation": 3}), fake.calls)
        self.assertNotIn("reply_release", names)

    def test_bad_signature_is_refused(self):
        runner = aw.Runner("https://example.test/mcp", "test-token", "custom", lambda c: None, webhook_secret=SECRET, transport=FakeMcp(lambda n, a: {}))
        body = event_body("reply.requested", "r1", "evt_1")
        self.assertEqual(runner.receive(signed(body, secret=OTHER), body), 401)

    def test_a_test_push_is_answered_and_claims_nothing(self):
        fake = FakeMcp(lambda n, a: {})
        runner = aw.Runner("https://example.test/mcp", "test-token", "custom", lambda c: None, webhook_secret=SECRET, transport=fake)
        body = json.dumps({"type": "ping", "id": "evt_ping", "timestamp": "now", "cursor": None, "data": {}})
        self.assertEqual(runner.receive(signed(body), body), 204)
        self.assertEqual(fake.calls, [])

    def test_the_listener_answers_pushes(self):
        import urllib.error
        import urllib.request

        runner = aw.Runner("https://example.test/mcp", "test-token", "custom", lambda c: None, webhook_secret=SECRET, transport=FakeMcp(lambda n, a: {}))
        server = runner.listen(0)
        try:
            body = event_body("reply.cancelled", "r9", "evt_9")
            url = f"http://127.0.0.1:{server.server_address[1]}/wake"
            with urllib.request.urlopen(urllib.request.Request(url, body.encode(), signed(body), method="POST")) as answer:
                self.assertEqual(answer.status, 204)
            with self.assertRaises(urllib.error.HTTPError) as caught:
                urllib.request.urlopen(urllib.request.Request(url, body.encode(), {}, method="POST"))
            self.assertEqual(caught.exception.code, 401)
        finally:
            server.shutdown()
            server.server_close()

    def test_an_error_releases_the_request(self):
        fake = FakeMcp(lambda name, args: request("r1", "go") if name == "reply_claim" else {"outcome": "ok", "value": {}})
        errors = []

        def on_request(context):
            raise RuntimeError("boom")

        runner = aw.Runner("https://example.test/mcp", "test-token", "custom", on_request, transport=fake, on_error=errors.append)
        runner.dispatch({"requestId": "r1"})
        self.assertEqual(fake.calls[-1], ("reply_release", {"requestId": "r1", "reason": "The runner hit an error and stopped.", "generation": 3}))
        self.assertEqual(len(errors), 1)


if __name__ == "__main__":
    unittest.main()

    def test_a_launched_request_reports_on_its_run(self):
        launched = request("r1", "go")
        launched["value"]["run"] = {"runId": "run-1", "state": "working"}
        plain = request("r2", "go")
        seen = {}

        def tools(name, args):
            if name == "reply_claim":
                return launched if args["requestId"] == "r1" else plain
            return {"outcome": "ok", "value": {}}

        fake = FakeMcp(tools)

        def on_request(context):
            seen[context.request_id] = context.run_id
            context.step("Reading the code")
            context.result("pull_request", "https://example.test/pr/1", "Fix it")
            context.reply("done")

        runner = aw.Runner("https://example.test/mcp", "test-token", "custom", on_request, transport=fake)
        runner.dispatch({"requestId": "r1"})
        runner.dispatch({"requestId": "r2"})
        self.assertEqual(seen, {"r1": "run-1", "r2": None})
        run_calls = [(name, args) for name, args in fake.calls if name.startswith("run_")]
        self.assertEqual([name for name, _ in run_calls], ["run_activity_report", "run_result_report"], "a request without a run reports nothing")
        activity = run_calls[0][1]["activities"][0]
        self.assertEqual((activity["kind"], activity["summary"]), ("step_started", "Reading the code"))
        self.assertEqual(run_calls[1][1], {"runId": "run-1", "results": [{"kind": "pull_request", "url": "https://example.test/pr/1", "title": "Fix it"}]})

    def test_session_bind_leaves_the_harness_out_when_there_is_none(self):
        fake = FakeMcp(lambda name, args: {"outcome": "ok", "value": {}})
        tools = aw.ReplyTools(aw.McpClient("https://example.test/mcp", "test-token", transport=fake))
        tools.session_bind("thread-1", "session-1")
        self.assertEqual(fake.calls[-1], ("session_bind", {"conversationId": "thread-1", "externalRef": "session-1"}))


class FakeOpened:
    def __init__(self, status, content_type, body):
        self.status = status
        self.headers = {"content-type": content_type}
        self._body = io.BytesIO(body.encode() if isinstance(body, str) else body)

    def readline(self):
        return self._body.readline()

    def read(self):
        return self._body.read()

    def close(self):
        pass


class FakeEvents:
    """The events address: the first read answers the fixture, later ones nothing."""

    def __init__(self, stream):
        self.stream = stream
        self.reads = []

    def __call__(self, url, headers, timeout):
        if url.endswith("/files/f1"):
            return FakeOpened(200, "text/plain", "hello")
        self.reads.append((headers.get("accept"), headers.get("last-event-id"), url))
        streamed = self.stream and headers.get("accept") == "text/event-stream"
        if len(self.reads) > 1:
            if streamed:
                return FakeOpened(200, "text/event-stream", ": stream open\n\n")
            return FakeOpened(200, "application/json", json.dumps({"events": [], "nextCursor": "c-none", "hasMore": False}))
        if streamed:
            return FakeOpened(200, "text/event-stream", fixture("stream.txt"))
        return FakeOpened(200, "application/json", fixture("poll-page.json"))


class ChannelTests(unittest.TestCase):
    CURSOR = "c1.AAAAAAAAAAAAAAAAAAAAAQAAAAAAAAAq.example"

    def read_with(self, stream):
        events = FakeEvents(stream)
        saved = []
        heard = []
        states = []

        class Store:
            def load(self):
                return None

            def save(self, cursor):
                saved.append(cursor)

        channel = aw.WorkspaceChannel(
            "https://aw.example", "test-token", None, cursor=Store(), opener=events, on_state=states.append
        )
        channel.on("message.*", heard.append)
        stop = threading.Event()
        thread = threading.Thread(target=channel.connect, args=(stop,))
        thread.start()
        deadline = time.time() + 5
        while time.time() < deadline and len(events.reads) < 2:
            time.sleep(0.01)
        stop.set()
        thread.join(5)
        self.assertEqual([event["type"] for event in heard], ["message.mentioned"])
        self.assertEqual(saved[0], self.CURSOR)
        accept, last_event_id, url = events.reads[1]
        if stream:
            self.assertEqual(last_event_id, self.CURSOR)
        else:
            self.assertIn("cursor=" + self.CURSOR, url)
        self.assertIn("streaming" if stream else "polling", states)
        self.assertEqual(states[-1], "closed")

    def test_reads_a_stream_once_each_and_keeps_its_place(self):
        self.read_with(stream=True)

    def test_long_polls_where_nothing_streams(self):
        self.read_with(stream=False)

    def test_talks_through_the_tools_and_fetches_files_by_link(self):
        answers = {
            "reply_send": {"outcome": "ok", "value": {"messageId": "m1"}},
            "message_send": {"outcome": "ok", "value": {"messageId": "m2"}},
            "message_edit": {"outcome": "ok", "value": {}},
            "message_react": {"outcome": "ok", "value": {}},
            "reply_progress": {"outcome": "ok", "value": {}},
            "file_link_get": {
                "outcome": "ok",
                "value": {"url": "https://aw.example/files/f1", "name": "note.txt", "contentType": "text/plain"},
            },
        }
        fake = FakeMcp(lambda name, args: answers.get(name, {"outcome": "not_found"}))
        channel = aw.WorkspaceChannel(
            "https://aw.example", "test-token", aw.McpClient("https://aw.example/api/v1/mcp", "test-token", fake), opener=FakeEvents(False)
        )
        self.assertEqual(
            channel.send("c", "done", reply={"requestId": "r1", "generation": 2}, client_message_id="mine-1", attachment_ids=["f9"]),
            "m1",
        )
        self.assertEqual(channel.send("c", "hello", client_message_id="mine-2"), "m2")
        channel.edit("m2", "hello again")
        channel.react("m2", "👀")
        channel.typing("r1", 2, "Reading")
        self.assertEqual(channel.download("f1"), ("note.txt", "text/plain", b"hello"))
        self.assertEqual(
            fake.calls,
            [
                ("reply_send", {"requestId": "r1", "generation": 2, "text": "done", "clientRequestId": "mine-1", "attachmentIds": ["f9"]}),
                ("message_send", {"conversationId": "c", "text": "hello", "clientRequestId": "mine-2"}),
                ("message_edit", {"messageId": "m2", "text": "hello again"}),
                ("message_react", {"messageId": "m2", "emoji": "👀", "reacted": True}),
                ("reply_progress", {"requestId": "r1", "generation": 2, "status": "Reading"}),
                ("file_link_get", {"fileId": "f1"}),
            ],
        )
        with self.assertRaises(aw.ToolError):
            channel.delete("m2")
