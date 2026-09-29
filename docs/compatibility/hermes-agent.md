# Hermes Agent compatibility

The first M9 connector targets Hermes Agent `0.20.0` from release
`v2026.8.3`, commit `3c27eb6234bf91b8ceee9e9071591b31e9b148cb`.
Compatibility is proved from an isolated Red Hat installation and profile; the
existing default Hermes `0.18.2` install and its state are not modified.

## Pinned installation

The Red Hat proof uses these immutable inputs and isolated paths:

- Hermes package version: `0.20.0` (`Hermes Agent v0.20.0 (2026.8.3)`)
- tag: `v2026.8.3`
- commit: `3c27eb6234bf91b8ceee9e9071591b31e9b148cb`
- annotated tag object: `7de39e700d2c329e15d32eb0b96e2f7cdd9fbdb2`
- source and environment root:
  `/home/bot/.local/share/agent-workspace/hermes/0.20.0`
- profile root: `/home/bot/.local/share/agent-workspace/hermes/profiles/m9`
- Python: `3.12.12`
- `uv`: `0.11.23` on `aarch64-unknown-linux-gnu`
- `uv.lock` SHA-256:
  `aab3c83f71b683507a590b6315b23bdc0abd6b63b76b2349eae15bf00dfbaf2b`
- normalized 70-line installed dependency fixture SHA-256:
  `cdd499d4ccdaa306d7e6ef1097f8efc79f40f60f58d1b118a4c5d15de55c7c9f`

The environment was created with `uv sync --frozen --no-dev --extra mcp`
against `/usr/bin/python3`, with `VIRTUAL_ENV` pointed at the isolated version
directory. The `mcp` extra is required for Streamable HTTP and pins
`mcp==1.28.1` plus the upstream `starlette==1.3.1` security override. The
normalized installed set is committed at
`tools/hermes-connector/fixtures/dependency-freeze.txt`. The profile,
`config.yaml`, profile home, and Codex-home placeholder are owned by `bot` and
mode `0700`/`0600` as applicable. `terminal.home_mode` is exactly `profile`.

Before installation and again after profile creation, the pre-existing command
remained `/home/bot/.local/bin/hermes`, reporting
`Hermes Agent v0.18.2 (2026.7.7.2)`. The M9 runtime never loaded the default
profile; it was not cloned, reconfigured, or modified by M9.

Official Hermes configuration supports remote HTTP MCP servers with `url` and
`headers`, profile-secret expansion through `${VAR}` or `${env:VAR}`, explicit
tool include/exclude filters, `supports_parallel_tool_calls`, and independent
resource, prompt, sampling, and elicitation controls. Registered tool names use
`mcp__<server>__<tool>`.

The M9 profile uses this contract:

```yaml
terminal:
  home_mode: profile

mcp_servers:
  agent_workspace:
    url: "http://127.0.0.1:13101/api/v1/mcp"
    headers:
      Authorization: "Bearer ${AGENT_WORKSPACE_MCP_TOKEN}"
    enabled: true
    timeout: 120
    connect_timeout: 30
    supports_parallel_tool_calls: false
    tools:
      include:
        - approval_get
        - approval_respond
        - conversation_list
        - conversation_read
        - external_event_poll
        - message_send
        - people_search
        - project_open
        - pull_request_get
        - repository_get
        - run_request
        - task_comment
        - task_create
        - task_list
        - task_mark_blocked
        - task_mark_ready_for_review
        - task_open
        - task_update
      resources: false
      prompts: false
    sampling:
      enabled: false
    elicitation:
      enabled: false
```

Hermes registers these as `mcp__agent_workspace__<tool>`. The allowlist is
also committed as `tools/hermes-connector/fixtures/tool-allowlist.txt` and CI
checks every entry against the Rust MCP implementation. The server advertises
`task_link_pull_request` to first-party human clients, but it is omitted
from this external profile because M9 does not delegate GitHub publication or
integration management.

Port 13101 is an operator-selected client-side SSH-forward port in the isolated
Hermes profile, not a new server listener. The server remains on its existing
loopback application boundary. A Mac-to-Red-Hat SSH reverse forward binds only
Red Hat `127.0.0.1:13101` and carries requests to the existing application
listener; a direct health probe through the forward succeeded before any MCP
credential was issued.

The live acceptance record below is completed only from redacted projections;
it never includes the model token, MCP bearer, browser session, Message body,
prompt, reasoning, repository contents, or filesystem paths returned by tools.

## Live connector acceptance

The isolated live profile authenticated its inference provider through the
personal ChatGPT OAuth ceremony and completed one-shot calls with
`gpt-5.6-terra`. A probe of `gpt-5.3-codex` returned HTTP 400 as unsupported;
that model is not part of this compatibility contract. No model credential was
copied into Agent Workspace.

Hermes initially lacked Streamable HTTP support when installed without extras.
Installing the pinned upstream `[mcp]` extra corrected the runtime boundary.
The resulting client negotiated MCP `2025-11-25` with
`agent-workspace/0.6.0`, discovered 23 server tools, and enabled the exact 22
tools above. An absent bearer returned HTTP 401. Live calls read the Project,
conversation, Actors, events, and Work Items; created and updated a Work Item;
and posted a Message attributed to the external Actor. The first Message call
also found that an unconstrained generated `content: true` schema was unusable
to Hermes. The server now publishes the bounded `MessageContent` schema and a
real Message call passes.

Event polling returned 18 addressed durable events. Starting from the opaque
cursor after sequence 10, two independent polls returned identical ordered
sequences 11 through 18 and the same next cursor. This proves at-least-once
replay without claiming exactly-once delivery. Cursors are signed and bound to
the connection, Project, and credential; the old cursor returned
`invalid_request` after credential rotation.

The browser ceremony used the authenticated personal Execution Profile. Hermes
created a sponsored Worker request, but no run or worktree existed until the
human sponsor selected **Approve and queue**. The accepted run retained the
Hermes Actor as requester and the human Actor as sponsor, allocated a unique
branch and isolated worktree from an exact base commit, and emitted command,
file, consolidated Worker Message, and terminal events. A second controlled
run was durably `working` when systemd restarted the application. Startup
reconciliation inspected it and produced an actionable `interrupted` Recovery
entry while preserving the same branch, base commit, and worktree without
database or filesystem repair.

The credential ceremony passed every transition: rotation made the previous
bearer return HTTP 401 while the replacement returned 200; disabling the
connection returned 401; re-enabling issued a new one-time bearer that returned
200; removing the connection made that final bearer return 401. Revoked bearer
files were scrubbed and the profile retains only an empty owner-only `.env`.
The committed evidence contains outcome labels and placeholders only.

The existing `/home/bot/.local/bin/hermes` remains version `0.18.2`. Its
long-running pre-existing `ash` gateway continued to update its own default
profile during acceptance, so a whole-profile metadata digest was not a valid
static comparison. M9 did not stop, reconfigure, clone, or write that profile;
the isolated source, environment, configuration, OAuth, and session roots all
remain beneath the dedicated Agent Workspace Hermes path. `/usr/bin/node`
remains `v22.22.2`, `/usr/bin/codex` remains `0.139.0`, the Agent Workspace
PostgreSQL container remains isolated on loopback port 55432, and unrelated
Podman projects were left running.

The normalized redacted result is committed at
`tools/hermes-connector/fixtures/live-evidence.json`.

Primary references:

- <https://github.com/NousResearch/hermes-agent/releases/tag/v2026.8.3>
- <https://github.com/NousResearch/hermes-agent/blob/main/website/docs/reference/mcp-config-reference.md>
- <https://github.com/NousResearch/hermes-agent/blob/main/website/docs/user-guide/configuration.md>
