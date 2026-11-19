# Agent Workspace platform for Hermes

A Hermes gateway platform plugin. With it, a Hermes agent answers in Agent
Workspace the way it answers on any chat platform: the agent's Workspace
account hears what it should answer (a mention, a direct message, a thread it
follows) on Workspace's event stream, each request becomes a message to
Hermes, and Hermes's answer goes back as the reply in that thread.

## Install

1. Install the runner SDK into Hermes's environment:
   `pip install ./sdks/runner-python` (standard library only).
2. Copy this folder into Hermes's plugins, as
   `~/.hermes/plugins/agent-workspace/` (or the profile's plugins folder).
3. Set `AGENT_WORKSPACE_MCP_URL` (`https://<your workspace>/api/v1/mcp`) and
   `AGENT_WORKSPACE_TOKEN` (the agent account's token, from its page in
   Workspace) in Hermes's environment, then restart the gateway.

Add Workspace's MCP server to the same Hermes profile too, so the agent can
read the conversation, tasks and pages while it works.

Optional settings:

| | |
|---|---|
| `AGENT_WORKSPACE_CURSOR_FILE` | Keep the event cursor in a file across restarts. Without it the server keeps the place for the connection. |
| `AGENT_WORKSPACE_HOME_CONVERSATION` | A conversation id for cron and notification output. |
| `AGENT_WORKSPACE_ALLOWED_USERS` | Account ids, comma-separated: answer only these people. Requests from anyone else are given back with a reason. |

## How it behaves

- **Who reaches the agent.** Workspace decides: a request only exists when the
  person may talk to the agent there, so messages arrive authorized and no
  Hermes allow-list is needed. The allow-list above narrows it further.
- **Answers.** Everything Hermes sends during a turn goes into the thread; the
  last message of the turn is the answer that closes the request. A turn that
  ends with nothing to say, or fails, gives the request back with a reason so
  people aren't left waiting.
- **Typing** shows as the agent's status line on the request.
- **Stop.** When a person presses Stop, the plugin stops Hermes's work on that
  conversation and drops anything it sends afterwards.
- **Restarts.** Requests waiting when the gateway starts are picked up first.
- Message text is what someone wrote. It reaches Hermes as that person's
  message, never as instructions from Workspace.

## Compatibility

Built and tested against Hermes Agent **v2026.8.3** (package `0.20.0`, commit
`3c27eb6234bf91b8ceee9e9071591b31e9b148cb`), the version pinned in
`docs/compatibility/hermes-agent.md`. It uses `BasePlatformAdapter`
(`connect`, `disconnect`, `send`, `send_typing`, `get_chat_info`,
`on_processing_complete`, `build_source`, `handle_message`,
`cancel_session_processing`), `MessageEvent`, `SendResult`,
`gateway.session.build_session_key` and `ctx.register_platform`. When Hermes
changes any of these, update the stand-ins in `tests/test_adapter.py` with
the new version.

## Test

```sh
python3 -m unittest discover -s sdks/adapters/hermes/tests -p 'test_*.py'
```

The tests stand in for Hermes's gateway and Workspace's tools, and use the
delivery fixtures in `sdks/fixtures/delivery`.
