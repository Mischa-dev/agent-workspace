#!/usr/bin/env python3
"""Verify the pinned, redacted Hermes M9 compatibility fixtures."""

from __future__ import annotations

import hashlib
import json
import re
from pathlib import Path


ROOT = Path(__file__).resolve().parents[2]
FIXTURES = ROOT / "tools" / "hermes-connector" / "fixtures"
COMPATIBILITY_NOTE = ROOT / "docs" / "compatibility" / "hermes-agent.md"
MCP_SOURCE = ROOT / "apps" / "server" / "src" / "mcp.rs"

EXPECTED_DEPENDENCY_SHA256 = (
    "cdd499d4ccdaa306d7e6ef1097f8efc79f40f60f58d1b118a4c5d15de55c7c9f"
)
EXPECTED_ALLOWLIST = (
    "approval_get",
    "approval_respond",
    "conversation_list",
    "conversation_message_send",
    "conversation_timeline_get",
    "external_event_poll",
    "external_execution_request_create",
    "mentionable_actor_search",
    "project_get",
    "project_instructions_get",
    "pull_request_get",
    "repository_get",
    "work_item_context_get",
    "work_item_create",
    "work_item_create_subtask",
    "work_item_get",
    "work_item_mark_blocked",
    "work_item_mark_ready_for_review",
    "work_item_post_update",
    "work_item_search",
    "work_item_set_status",
    "work_item_update",
)
FORBIDDEN_FIXTURE_PATTERNS = (
    re.compile(r"aw_ext_[A-Za-z0-9_-]{12,}"),
    re.compile(r"Bearer\s+[A-Za-z0-9_-]{12,}", re.IGNORECASE),
    re.compile(r'"(?:access|refresh|id)_token"\s*:', re.IGNORECASE),
    re.compile(r"[A-Za-z0-9._%+-]+@gmail\.com", re.IGNORECASE),
    re.compile(r"/home/bot/"),
)


def fail(message: str) -> None:
    raise SystemExit(message)


def main() -> None:
    dependency_path = FIXTURES / "dependency-freeze.txt"
    dependency_bytes = dependency_path.read_bytes()
    dependency_lines = dependency_path.read_text(encoding="utf-8").splitlines()
    if len(dependency_lines) != 70:
        fail("Hermes dependency fixture must contain exactly 70 lines")
    if dependency_lines[0] != "hermes-agent @ file://<isolated-source>":
        fail("Hermes editable source path is not normalized")
    if "mcp==1.28.1" not in dependency_lines:
        fail("Hermes Streamable HTTP MCP extra is not pinned")
    if "starlette==1.3.1" not in dependency_lines:
        fail("Hermes MCP transitive security override is not pinned")
    dependency_sha256 = hashlib.sha256(dependency_bytes).hexdigest()
    if dependency_sha256 != EXPECTED_DEPENDENCY_SHA256:
        fail("Hermes dependency fixture checksum changed")

    allowlist = tuple(
        line
        for line in (FIXTURES / "tool-allowlist.txt")
        .read_text(encoding="utf-8")
        .splitlines()
        if line
    )
    if allowlist != EXPECTED_ALLOWLIST:
        fail("Hermes tool allowlist changed or is not sorted")
    if len(set(allowlist)) != len(allowlist):
        fail("Hermes tool allowlist contains duplicates")
    mcp_source = MCP_SOURCE.read_text(encoding="utf-8")
    for tool_name in allowlist:
        if not re.search(rf"async fn {re.escape(tool_name)}\s*\(", mcp_source):
            fail(f"Hermes allowlisted tool is missing from MCP: {tool_name}")
    if "work_item_link_pull_request" in allowlist:
        fail("GitHub publication management must remain outside the Hermes allowlist")

    compatibility_note = COMPATIBILITY_NOTE.read_text(encoding="utf-8")
    if EXPECTED_DEPENDENCY_SHA256 not in compatibility_note:
        fail("Hermes compatibility note does not record the dependency checksum")

    for fixture_path in sorted(FIXTURES.iterdir()):
        if not fixture_path.is_file():
            continue
        fixture_text = fixture_path.read_text(encoding="utf-8")
        for pattern in FORBIDDEN_FIXTURE_PATTERNS:
            if pattern.search(fixture_text):
                fail(f"Hermes fixture contains forbidden sensitive material: {fixture_path.name}")

    live_evidence = FIXTURES / "live-evidence.json"
    if live_evidence.exists():
        payload = json.loads(live_evidence.read_text(encoding="utf-8"))
        if payload.get("fixtureVersion") != 1:
            fail("Hermes live evidence fixtureVersion must be 1")
        if payload.get("redacted") is not True:
            fail("Hermes live evidence must declare redacted=true")

    print("Hermes connector fixtures are pinned and redacted")


if __name__ == "__main__":
    main()
