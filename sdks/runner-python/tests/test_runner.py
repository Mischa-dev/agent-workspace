import json
import threading
import time
import unittest

import agent_workspace_runner as aw
from agent_workspace_runner import HttpResponse

SECRET = "test-webhook-secret"


def signed(body, timestamp=None, secret=SECRET):
    timestamp = str(int(time.time()) if timestamp is None else timestamp)
    return {
        "x-workspace-timestamp": timestamp,
        "x-workspace-signature": aw.sign_wakeup(secret, timestamp, body),
        "x-workspace-delivery": json.loads(body)["deliveryId"],
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


class WakeupTests(unittest.TestCase):
    def test_genuine_wakeup_verifies(self):
        body = json.dumps({"event": "reply.requested", "deliveryId": "d1", "agentId": "a", "replyRequestId": "r1"})
        self.assertEqual(aw.verify_wakeup(SECRET, signed(body), body)["replyRequestId"], "r1")

    def test_tampered_stale_unsigned_or_wrong_secret_fail(self):
        body = json.dumps({"event": "reply.requested", "deliveryId": "d1", "agentId": "a", "replyRequestId": "r1"})
        with self.assertRaises(aw.WakeupError):
            aw.verify_wakeup(SECRET, signed(body), body.replace("r1", "r2"))
        with self.assertRaises(aw.WakeupError):
            aw.verify_wakeup(SECRET, signed(body, timestamp=int(time.time()) - 301), body)
        with self.assertRaises(aw.WakeupError):
            aw.verify_wakeup(SECRET, {}, body)
        with self.assertRaises(aw.WakeupError):
            aw.verify_wakeup(SECRET, signed(body, secret="other"), body)

    def test_deliveries_are_handled_once(self):
        deduper = aw.DeliveryDeduper(limit=2)
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
            body = json.dumps({"event": event, "deliveryId": delivery, "agentId": "a", "replyRequestId": request_id, **extra})
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
        body = json.dumps({"event": "reply.requested", "deliveryId": "d1", "agentId": "a", "replyRequestId": "r1"})
        self.assertEqual(runner.receive(signed(body, secret="other"), body), 401)

    def test_a_test_wakeup_is_answered_and_claims_nothing(self):
        fake = FakeMcp(lambda n, a: {})
        runner = aw.Runner("https://example.test/mcp", "test-token", "custom", lambda c: None, webhook_secret=SECRET, transport=fake)
        body = json.dumps({"event": "ping", "deliveryId": "d1", "agentId": "a", "projectId": "p"})
        self.assertEqual(runner.receive(signed(body), body), 204)
        self.assertEqual(fake.calls, [])

    def test_the_listener_answers_wakeups(self):
        import urllib.error
        import urllib.request

        runner = aw.Runner("https://example.test/mcp", "test-token", "custom", lambda c: None, webhook_secret=SECRET, transport=FakeMcp(lambda n, a: {}))
        server = runner.listen(0)
        try:
            body = json.dumps({"event": "reply.cancelled", "deliveryId": "d9", "agentId": "a", "replyRequestId": "r9"})
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
        runner.dispatch({"event": "reply.requested", "replyRequestId": "r1"})
        self.assertEqual(fake.calls[-1], ("reply_release", {"requestId": "r1", "reason": "The runner hit an error and stopped.", "generation": 3}))
        self.assertEqual(len(errors), 1)


if __name__ == "__main__":
    unittest.main()
