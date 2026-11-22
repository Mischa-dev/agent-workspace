"""The Hermes plugin against a stand-in gateway and an in-memory Workspace.

Hermes itself is not installed here: the few gateway pieces the plugin uses
are stubbed with the shapes of Hermes v2026.8.3 (see README.md). Events are
the delivery fixtures' envelope, as the stream carries it.
"""

import asyncio
import copy
import enum
import importlib.util
import json
import os
import sys
import threading
import types
import unittest
from dataclasses import dataclass
from typing import Any, Optional

HERE = os.path.dirname(os.path.abspath(__file__))
PLUGIN = os.path.dirname(HERE)
ROOT = os.path.dirname(os.path.dirname(PLUGIN))
sys.path.insert(0, os.path.join(ROOT, "runner-python"))

with open(os.path.join(ROOT, "fixtures", "delivery", "envelope.json"), encoding="utf-8") as file:
    ENVELOPE = json.load(file)


# ---------------------------------------------------------------------------
# A stand-in for Hermes's gateway, with the shapes the plugin relies on.


def _install_gateway():
    gateway = types.ModuleType("gateway")
    config = types.ModuleType("gateway.config")
    platforms = types.ModuleType("gateway.platforms")
    base = types.ModuleType("gateway.platforms.base")
    session = types.ModuleType("gateway.session")

    class Platform(str):
        pass

    class MessageType(enum.Enum):
        TEXT = "text"

    class ProcessingOutcome(enum.Enum):
        SUCCESS = "success"
        FAILURE = "failure"
        CANCELLED = "cancelled"

    @dataclass
    class SendResult:
        success: bool
        message_id: Optional[str] = None
        error: Optional[str] = None
        raw_response: Any = None
        retryable: bool = False

    @dataclass
    class MessageEvent:
        text: str
        message_type: Any = None
        source: Any = None
        raw_message: Any = None
        message_id: Optional[str] = None
        timestamp: Any = None

    @dataclass
    class SessionSource:
        platform: Any
        chat_id: str
        chat_name: Optional[str] = None
        chat_type: str = "dm"
        user_id: Optional[str] = None
        user_name: Optional[str] = None
        thread_id: Optional[str] = None
        parent_chat_id: Optional[str] = None
        message_id: Optional[str] = None
        role_authorized: bool = False

    class BasePlatformAdapter:
        def __init__(self, config, platform):
            self.config = config
            self.platform = platform
            self.handled = []
            self.cancelled = []
            self.connected = False
            self.fatal = None

        def _mark_connected(self):
            self.connected = True

        def _mark_disconnected(self):
            self.connected = False

        def _set_fatal_error(self, code, message, *, retryable):
            self.fatal = (code, message, retryable)

        def build_source(self, chat_id, chat_name=None, chat_type="dm", user_id=None, user_name=None,
                         thread_id=None, parent_chat_id=None, message_id=None, role_authorized=False, **_):
            return SessionSource(self.platform, chat_id, chat_name, chat_type, user_id, user_name,
                                 thread_id, parent_chat_id, message_id, role_authorized)

        async def handle_message(self, event):
            self.handled.append(event)

        async def cancel_session_processing(self, session_key, **_):
            self.cancelled.append(session_key)

    def build_session_key(source, group_sessions_per_user=True, thread_sessions_per_user=False):
        return f"agent:main:{source.platform}:{source.chat_type}:{source.chat_id}"

    config.Platform = Platform
    base.BasePlatformAdapter = BasePlatformAdapter
    base.MessageEvent = MessageEvent
    base.MessageType = MessageType
    base.SendResult = SendResult
    base.ProcessingOutcome = ProcessingOutcome
    session.build_session_key = build_session_key
    gateway.config, gateway.platforms, platforms.base, gateway.session = config, platforms, base, session
    sys.modules.update({
        "gateway": gateway,
        "gateway.config": config,
        "gateway.platforms": platforms,
        "gateway.platforms.base": base,
        "gateway.session": session,
    })
    return ProcessingOutcome


Outcome = _install_gateway()


def _load_plugin():
    """Loads the plugin the way Hermes does: a package from its directory."""
    spec = importlib.util.spec_from_file_location(
        "hermes_plugins.agent_workspace", os.path.join(PLUGIN, "__init__.py"), submodule_search_locations=[PLUGIN]
    )
    module = importlib.util.module_from_spec(spec)
    sys.modules[spec.name] = module
    spec.loader.exec_module(module)
    return sys.modules["hermes_plugins.agent_workspace.adapter"], module


adapter_module, plugin = _load_plugin()

CONVERSATION = ENVELOPE["data"]["conversationId"]
MESSAGE = ENVELOPE["data"]["messageId"]
ASKER = ENVELOPE["actor"]["accountId"]


def reply_event(kind, request_id, event_id):
    event = copy.deepcopy(ENVELOPE)
    event["type"] = kind
    event["id"] = event_id
    event["reason"] = "reply_required"
    event["data"]["thing"] = {"kind": "reply_request", "id": request_id}
    return event


class FakeWorkspace:
    """The MCP tools the plugin calls, answered from memory."""

    def __init__(self, waiting=()):
        self.calls = []
        self.waiting = list(waiting)
        # Calls to fail before answering, by tool name: a network failure.
        self.failing = {}

    def call_tool(self, name, arguments):
        self.calls.append((name, dict(arguments)))
        if self.failing.get(name, 0) > 0:
            self.failing[name] -= 1
            raise ConnectionError("the server went away")
        if name == "inbox_list":
            return ok({"requests": self.waiting})
        if name == "reply_claim":
            return ok(
                {
                    "request": {
                        "requestId": arguments["requestId"],
                        "generation": 2,
                        "conversationId": CONVERSATION,
                        "replyConversationId": CONVERSATION,
                        "conversationKind": "channel",
                        "conversationName": "launch",
                        "messageId": MESSAGE,
                        "messageAuthor": "Ada",
                        "messageExcerpt": "@helper what's left?",
                        "threadId": None,
                    },
                    "recentMessages": [
                        {"messageId": MESSAGE, "author": "Ada", "authorId": ASKER, "text": "@helper what's left?"}
                    ],
                    "instructions": "",
                    "session": None,
                }
            )
        if name in ("reply_send", "message_send"):
            return ok({"messageId": f"m{len(self.calls)}"})
        return ok({})

    def named(self, name):
        return [arguments for called, arguments in self.calls if called == name]


def ok(value):
    return {"outcome": "ok", "value": value}


class FakeChannel:
    def __init__(self):
        self.handlers = []
        self.connected = asyncio.Event()

    def on(self, pattern, handler):
        self.handlers.append((pattern, handler))
        return self

    def connect(self, stop):
        stop.wait(5)


class Config:
    extra = {}


class PluginTest(unittest.IsolatedAsyncioTestCase):
    async def asyncSetUp(self):
        self.workspace = FakeWorkspace()
        self.channel = FakeChannel()
        self.adapter = adapter_module.AgentWorkspaceAdapter(Config(), tools=self.workspace, channel=self.channel)
        self.assertTrue(await self.adapter.connect())

    async def asyncTearDown(self):
        await self.adapter.disconnect()

    async def hear(self, event):
        for _, handler in self.channel.handlers:
            handler(event)
        for _ in range(50):
            await asyncio.sleep(0.01)
            if not any(task for task in asyncio.all_tasks() if task is not asyncio.current_task() and not task.done()):
                break
        await asyncio.sleep(0.05)

    async def test_a_request_becomes_one_message_from_the_person_who_asked(self):
        await self.hear(reply_event("reply.requested", "r1", "e1"))
        await self.hear(reply_event("reply.requested", "r1", "e1-again"))

        self.assertEqual(len(self.workspace.named("reply_claim")), 1)
        [event] = self.adapter.handled
        self.assertEqual(event.text, "what's left?")
        self.assertEqual(event.message_id, "r1")
        self.assertEqual(event.source.chat_id, CONVERSATION)
        self.assertEqual(event.source.chat_type, "group")
        self.assertEqual(event.source.user_id, ASKER)
        self.assertEqual(event.source.user_name, "Ada")
        self.assertTrue(event.source.role_authorized)

    async def test_a_request_named_twice_at_once_is_claimed_once(self):
        await asyncio.gather(self.adapter._take("r9"), self.adapter._take("r9"))
        self.assertEqual(len(self.workspace.named("reply_claim")), 1)

    async def test_the_last_message_of_the_turn_is_the_answer(self):
        await self.hear(reply_event("reply.requested", "r1", "e1"))
        await self.adapter.send_typing(CONVERSATION)
        first = await self.adapter.send(CONVERSATION, "Looking now.")
        second = await self.adapter.send(CONVERSATION, "Two tasks are left.")
        self.assertTrue(first.success and second.success)
        await self.adapter.on_processing_complete(self.adapter.handled[0], Outcome.SUCCESS)

        sends = self.workspace.named("reply_send")
        self.assertEqual([(s["text"], s["final"]) for s in sends], [("Looking now.", False), ("Two tasks are left.", True)])
        self.assertTrue(all(s["requestId"] == "r1" and s["generation"] == 2 for s in sends))
        self.assertEqual(self.workspace.named("reply_progress_report")[0]["requestId"], "r1")
        self.assertEqual(self.workspace.named("reply_release"), [])

        # With nothing held, a send is an ordinary message.
        await self.adapter.send(CONVERSATION, "Daily summary")
        self.assertEqual(self.workspace.named("message_send")[0]["text"], "Daily summary")

    async def test_an_answer_whose_send_fails_is_sent_again_unchanged(self):
        await self.hear(reply_event("reply.requested", "r1", "e1"))
        await self.adapter.send(CONVERSATION, "Two tasks are left.")
        self.workspace.failing["reply_send"] = 1
        await self.adapter.on_processing_complete(self.adapter.handled[0], Outcome.SUCCESS)

        sends = self.workspace.named("reply_send")
        self.assertEqual([(s["text"], s["final"]) for s in sends], [("Two tasks are left.", True)] * 2)
        self.assertEqual(sends[0]["clientRequestId"], sends[1]["clientRequestId"])
        self.assertEqual(self.workspace.named("reply_release"), [])

    async def test_a_follow_up_waits_for_the_turn_before_it(self):
        await self.hear(reply_event("reply.requested", "r1", "e1"))
        await self.hear(reply_event("reply.requested", "r2", "e2"))
        # Claimed, but not handed over while r1 is being answered.
        self.assertEqual([event.message_id for event in self.adapter.handled], ["r1"])
        await self.adapter.send(CONVERSATION, "Two tasks are left.")
        await self.adapter.on_processing_complete(self.adapter.handled[0], Outcome.SUCCESS)
        await self.hear({})

        sends = self.workspace.named("reply_send")
        self.assertEqual([(s["requestId"], s["text"]) for s in sends], [("r1", "Two tasks are left.")])
        self.assertEqual(self.workspace.named("reply_release"), [])
        self.assertEqual([event.message_id for event in self.adapter.handled], ["r1", "r2"])

    async def test_a_turn_with_no_answer_or_an_error_gives_the_request_back(self):
        await self.hear(reply_event("reply.requested", "r1", "e1"))
        await self.adapter.on_processing_complete(self.adapter.handled[0], Outcome.SUCCESS)
        await self.hear(reply_event("reply.requested", "r2", "e2"))
        await self.adapter.send(CONVERSATION, "Partial")
        await self.adapter.on_processing_complete(self.adapter.handled[1], Outcome.FAILURE)

        releases = self.workspace.named("reply_release")
        self.assertEqual([r["requestId"] for r in releases], ["r1", "r2"])
        self.assertEqual(self.workspace.named("reply_send")[0]["final"], False)

    async def test_stop_ends_the_work_and_sends_nothing_more(self):
        await self.hear(reply_event("reply.requested", "r1", "e1"))
        await self.hear(reply_event("reply.cancelled", "r1", "e2"))

        self.assertEqual(len(self.adapter.cancelled), 1)
        result = await self.adapter.send(CONVERSATION, "Too late")
        await self.adapter.on_processing_complete(self.adapter.handled[0], Outcome.CANCELLED)
        self.assertFalse(result.success)
        self.assertEqual(self.workspace.named("reply_send"), [])
        self.assertEqual(self.workspace.named("message_send"), [])
        self.assertEqual(self.workspace.named("reply_release"), [])

    async def test_an_allow_list_releases_requests_from_anyone_else(self):
        self.adapter.allowed_users = {"someone-else"}
        await self.hear(reply_event("reply.requested", "r1", "e1"))
        self.assertEqual(self.adapter.handled, [])
        self.assertEqual([r["requestId"] for r in self.workspace.named("reply_release")], ["r1"])

    async def test_requests_waiting_at_startup_are_picked_up(self):
        workspace = FakeWorkspace(waiting=[{"requestId": "w1", "status": "pending"}, {"requestId": "w2", "status": "done"}])
        adapter = adapter_module.AgentWorkspaceAdapter(Config(), tools=workspace, channel=FakeChannel())
        await adapter.connect()
        await asyncio.sleep(0.1)
        await adapter.disconnect()
        self.assertEqual([c["requestId"] for c in workspace.named("reply_claim")], ["w1"])


    async def test_past_one_page_each_finished_turn_reads_the_inbox_again(self):
        backlog = [{"requestId": f"w{i}", "status": "pending"} for i in range(adapter_module.INBOX_PAGE + 3)]
        answered = set()
        workspace = FakeWorkspace()

        def inbox(name, arguments, call=workspace.call_tool):
            if name == "inbox_list":
                workspace.calls.append((name, dict(arguments)))
                left = [item for item in backlog if item["requestId"] not in answered]
                return ok({"requests": left[: arguments.get("limit", 20)]})
            return call(name, arguments)

        workspace.call_tool = inbox
        adapter = adapter_module.AgentWorkspaceAdapter(Config(), tools=workspace, channel=FakeChannel())
        await adapter.connect()
        # One conversation answers one request at a time.
        for _ in range(400):
            await asyncio.sleep(0.005)
            for event in list(adapter.handled):
                if event.message_id not in answered:
                    answered.add(event.message_id)
                    await adapter.on_processing_complete(event, Outcome.SUCCESS)
            if len(answered) == len(backlog):
                break
        await adapter.disconnect()

        self.assertEqual(workspace.named("inbox_list")[0]["limit"], adapter_module.INBOX_PAGE)
        self.assertEqual(len(answered), len(backlog))

    async def test_a_stop_while_the_claim_is_on_its_way_starts_no_turn(self):
        claim_answered = threading.Event()
        call = self.workspace.call_tool

        def slow_claim(name, arguments):
            if name == "reply_claim":
                claim_answered.wait(2)
            return call(name, arguments)

        self.workspace.call_tool = slow_claim
        taking = asyncio.ensure_future(self.adapter._take("r1"))
        await asyncio.sleep(0.05)
        await self.adapter._cancelled("r1")
        claim_answered.set()
        await taking

        self.assertEqual(self.adapter.handled, [])
        self.assertEqual(self.workspace.named("reply_send"), [])

class RegistrationTest(unittest.TestCase):
    def test_register_names_the_platform_and_its_settings(self):
        registered = {}

        class Context:
            def register_platform(self, **kwargs):
                registered.update(kwargs)

        plugin.register(Context())
        self.assertEqual(registered["name"], "agent_workspace")
        self.assertEqual(registered["required_env"], ["AGENT_WORKSPACE_MCP_URL", "AGENT_WORKSPACE_TOKEN"])
        self.assertIsInstance(registered["adapter_factory"](Config()), adapter_module.AgentWorkspaceAdapter)

    def test_the_manifest_matches_the_registration(self):
        with open(os.path.join(PLUGIN, "plugin.yaml"), encoding="utf-8") as file:
            manifest = file.read()
        self.assertIn("kind: platform", manifest)
        for name in ("AGENT_WORKSPACE_MCP_URL", "AGENT_WORKSPACE_TOKEN"):
            self.assertIn(f"- name: {name}", manifest)


if __name__ == "__main__":
    unittest.main()
