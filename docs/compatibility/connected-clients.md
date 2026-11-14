# Connected clients

Some clients connect to Agent Workspace and can't be updated with it: they
keep calling what they called when they were built. Each contract below is
pinned by `apps/server/tests/fixtures/compat/connected-clients.json` and
checked by `apps/server/tests/integration/compat_clients.rs`. Change a
contract only when its retirement condition holds, and change the fixture,
this page and the code in the same pull request.

| Contract | Relied on by | Checked by | Retire when | Owner |
| --- | --- | --- | --- | --- |
| The `inbox.available` wake format (`event`, `deliveryId`, `agentId`, `replyRequestId`) for webhooks saved before typed wake-ups | The always-on runtime | `reply_requests::webhooks_say_what_happened_without_message_text` | A2's typed event stream has shipped and the always-on runtime accepts typed events | A2 |
| MCP revisions `2025-06-18` and `2025-11-25`: `initialize` answers the same `protocolVersion` and an `mcp-session-id`; a session the server doesn't hold resumes | The always-on runtime, Hermes 0.20, connected apps | `compat_clients::old_mcp_revisions_still_shake_hands` | Not before 12 months after K2 serves `2026-07-28`, and only with the owner's sign-off | K2 |
| The tools each client calls, under their names, with the argument names it sends, each answering with an `outcome` | The always-on runtime (the runner contract, `docs/agent-runner.md`); Hermes 0.20 (`tools/hermes-connector/fixtures/tool-allowlist.txt`) | `compat_clients::connected_clients_find_their_tools` | The client has moved to the new name or argument; K2's revision may rename only behind the old name | K2 |
| The URLs agents are pointed at: the MCP endpoint (401 with `resource_metadata` without a token), its OAuth metadata, `/agent-runner.md`, `/connect.md`, the public API's OpenAPI document | Every connected agent and app | `compat_clients::agent_facing_urls_answer` | Never removed; a move keeps a redirect from the old URL | K2 |

## Adding a contract

When a client starts relying on something new (a tool, an argument, a URL),
add it to the fixture with the client that relies on it. The fixture names
no person and no machine.
