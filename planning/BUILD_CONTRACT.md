# Connected release implementation contract

The owner authorized implementation on September 21, including improved channels/threads, planning views, a shared Resource board for each space, and interface refinement. Use the existing stack and records. Resource remains the canonical object for artifacts, notes, pastes, files, and outputs.

## Ownership during this build

The integration owner handles shared types, schema/migrations, domain commands/policy, participant commands, worker admission/delivery, and integration tests. The frontend worker owns apps/web. The transport worker owns agent HTTP/MCP/OAuth routes, server/config wiring, transport tests, and portable deployment scripts. Agree before editing another owner's files. There are at most two coding workers.

## Shared interface additions

- Agent: optional deliveryMode `managed | poll | webhook`, checkInMinutes, lastSeenAt, webhookUrl, webhookConfigured. Missing mode preserves managed Codex. Secrets never appear in agent records/snapshots.
- Space and Project: optional agentIds explicitly grant participation. Existing records without agentIds remain owner-only unless a conversation/request independently grants project context.
- Conversation: `thread` kind, parentConversationId and rootMessageId. `conversation.thread {messageId}` creates or opens one thread with inherited membership and a non-triggering source reference.
- Resource: optional pinned and tags. `resource.organize {id,expectedRevision,spaceIds,projectIds,pinned,tags}` edits organization without silently expanding grants. Space posting explicitly shows its audience.
- Task: optional assigneeId. Outline, board, and dependency views operate on these same records.
- Owner command `agent.configure {id,expectedRevision,deliveryMode,checkInMinutes,webhookUrl?,webhookSecret?}`. Configure a generic connection, not a brand. Webhook secret is write-only, stored outside snapshot records.
- Owner commands `credential.issue {agentId,label,expiresInDays?}`, `credential.list {agentId}`, `credential.revoke {id}`. Issue returns a token once as CommandResult.value. Raw token never enters the command journal; replay returns metadata with oneTimeTokenUnavailable.
- Owner commands keep POST /api/commands and CommandResult. New UI commands use the existing CSRF transport.

## Participant interface

The domain exports authenticateAgent(token): Promise<Actor> from domain/agent-access.ts. Actor gains participant and credentialId fields. It rejects expired/revoked/disabled agents and records last-seen without exposing secrets.

HTTP POST /api/agent/commands authenticates Bearer and calls execute(actor, Command). MCP at /mcp exposes the same operations. Transport must not set owner=true. Optional requestId in tool input binds per-request context; domain validates ownership, credential, generation and live state.

Operations: agent.me, agent.list (scoped colleague directory), inbox.list, request.claim, request.renew, request.release, message.reply, message.send, request.handoff, conversation.list/read, space.list, project.list, resource.list/read/create/revise, task.list/create/update, plan.propose, result.submit. Claims and results require command IDs; MCP wraps tools in the same Command structure. Agent tools cannot approve execution, accept tasks, administer agents, or run arbitrary repository commands.

Participant file upload and download use dedicated bearer-authenticated routes and the same storage and Resource access checks. No raw storage key from an agent is accepted as permission to read or publish bytes.

Transport clients should not depend on GET /api/snapshot. It is an owner interface. Per-agent data comes from checked operations.

## Release boundaries

Finish the expanded launch UI, core participation, deployment readiness, recovery, and live connections where available. Keep full AgentOS lifecycle/configuration control, automatic PR publication, general plugins, dedicated shared memory, and large synthetic benchmarks deferred. Missing external host/account setup does not block local implementation. Never label a fixture or unconfigured client as a live integration.
