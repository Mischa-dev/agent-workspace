# Connected workspace release

September 21, 2026. Implemented from the expanded launch authorization. The application is deployed at https://workspace.mischanelson.dev on Red Hat through a dedicated Cloudflare tunnel. Public OAuth/MCP, file handoff, daily backups and isolated restore are verified. The owner's actual third-party client connections remain pending.

## Delivered

- Spaces have persistent boards for notes, pastes, uploaded files and agent outputs. Pin and tag material, organize it across spaces/projects, open revision history, and attach the same Resource to messages or tasks. Sharing is explicit; organization alone does not grant access.
- Conversations include DMs, groups, channels and message threads. Threads inherit participation, and parent membership edits propagate. Existing recipient selection, forwarding, drafts, Stop and managed independent opinions remain.
- Projects have an expandable outline, status board, dependency view, assignment and existing proposal/scope flows. Assigned ready chat tasks appear in the agent's inbox. Code execution stays within the existing owner-approved local profile and respects the selected local agent.
- Any named agent can use a managed local runtime, check its inbox through MCP/HTTP, or receive a signed webhook hint and then claim work. These are editable connection settings, not branches for Muse, Hermes or another brand. A client owns its own heartbeat schedule.
- Revocable agent credentials, checked file endpoints, a real MCP server, OAuth consent with PKCE and rotating refresh credentials all use the same domain rules. Agents can read permitted context, save Resources, propose/edit drafts, reply and hand off work. They cannot administer the workspace, approve repository execution, or accept their own results.
- Claims are transactional and have a renewable 15-minute lease. A missed lease holds uncertain work rather than replaying it. Handoffs use the remaining owner allowance. Late results can be retained as evidence without completing stopped work. Webhook receipts do not claim or execute a request.
- The local migration and backup path preserves the existing workspace. Restore revokes agent access and OAuth state, pauses dispatch and holds uncertain attempts. A portable Podman deployment, persistent volumes and operational instructions are included under `deploy/`.

## Verification

See `ACCEPTANCE.md` and `evidence/` for final counts and reports. Domain tests use real isolated PostgreSQL; transport tests exercise the official MCP handler/client. A real loopback HTTP exercise uploads/downloads a file, hands it between two independent accounts and returns the result to the conversation. This is protocol qualification without an external model call.

Browser checks exercised the shared board, pinned note, explicit audience, channel attachment, thread source, project outline/board/dependencies and agent setup. The expanded desktop layout was visually inspected. Existing mobile acceptance predates this expansion; responsive rules are present but should not be described as a new physical-device test.

A full database-and-file backup was restored into a new database and directory, verified by record counts and file checksum, then restarted with the real server and worker. Dispatch remained paused and old work did not replay.

## Deployment and connection status

| Target | Current evidence | Remaining step |
| --- | --- | --- |
| Local application | Production build and original data preserved | Hosted workspace is now the primary URL; local launcher remains available for development |
| Managed local Codex | Prior real DM/group/planning/worktree qualification retained; regression tests pass | New remote hosts need their own runtime qualification |
| Generic MCP/HTTP client | Official client, OAuth renewal and cross-account file handoff verified over public HTTPS | Connect each actual client using its own agent account |
| Muse | Owner confirms custom MCP and editable heartbeat, no incoming webhook | Connect the reachable MCP endpoint in Muse and run its actual heartbeat |
| ChatGPT Work / other Codex client | Generic account and OAuth/MCP support implemented | Connect that actual client and verify scheduled tool access |
| Hermes / OpenClaw | Generic polling and signed wakeup contract implemented | Configure the intended runtime's supported receiver and verify its real pickup |
| Remote deployment | Live at workspace.mischanelson.dev; persistent volumes, service restart, daily backup and isolated restore verified | Monitor ordinary usage; no host reboot or large-scale load test was performed |

The owner authorized the Red Hat/Cloudflare deployment. A dedicated tunnel, hostname and workspace services are active. Other applications were not redeployed. No paid resource, repository push or merge was created. See `deploy/REDHAT.md` for operations and the precise remaining client setup.

## Kept out of this release

Full AgentOS configuration/lifecycle control, a general plugin/automation designer, automatic PR publication, arbitrary project test execution, advanced branch integration, a dedicated memory service and large-scale benchmarks remain deferred. Resources provide shared notes and files now. The ordinary-use timing sample is a small review workspace, not evidence of massive-roadmap performance.
