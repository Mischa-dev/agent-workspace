# External agent connection threat model

This document defines the M9 entry boundary. The first supported connector is
Hermes Agent `0.20.0`, release tag `v2026.8.3`, annotated tag object
`7de39e700d2c329e15d32eb0b96e2f7cdd9fbdb2`, commit
`3c27eb6234bf91b8ceee9e9071591b31e9b148cb`. The runtime remains externally
owned and is connected through the existing loopback Streamable HTTP MCP
endpoint over SSH forwarding.

## Protected assets and trust boundaries

Agent Workspace protects Project isolation, Actor attribution, Messages, Work
Items, approvals, repository/worktree identity, personal Execution Profiles,
GitHub credentials, browser sessions, provider caches, database credentials,
and audit history. Hermes controls its own model, prompts, memory, sessions,
schedules, skills, tools, terminal, and autonomy loop. Neither side treats the
other as a privileged database or filesystem client.

The connector is hostile-by-default input. A correct bearer proves only one
active external-agent Actor, one Project, one active connection, and explicit
scopes. The ordinary centralized action-on-resource policy then runs. Actor
kind never creates an authority bypass, clients never submit authorship, and
unknown or cross-Project identifiers remain non-disclosing.

## First protocol

- Agent Workspace exposes its existing MCP 2025-06-18 Streamable HTTP
  endpoint. The host remains loopback-only; the remote runtime reaches it only
  through an operator-controlled SSH tunnel.
- An owner or admin creates an external-agent member and a named connection.
  A 256-bit bearer is returned once; PostgreSQL stores only SHA-256 digest and
  bounded non-secret prefix metadata. Rotation is atomic and revokes the
  predecessor. Disable or removal revokes every connection credential.
- The initial Hermes profile sets `terminal.home_mode: profile`, uses a
  separate `HERMES_HOME`, and references the bearer from its profile-owned
  `.env`. Remote MCP configuration allowlists only Agent Workspace tools,
  disables resources, prompts, sampling, elicitation, and parallel calls, and
  never copies credentials from the host's default Hermes or Codex profiles.
- Connection presence is a bounded diagnostic derived from throttled
  `last_used_at`; it is not a liveness, availability, or personhood claim.
- Delivery is pull-based in M9: an opaque connection-bound cursor returns at
  most 100 durable addressed events. Delivery is at-least-once invalidation;
  the client refetches canonical state. Cursor expiry is explicit and falls
  back to bounded resynchronization. No permanent public webhook is opened.

## Threats and required controls

| Threat | Control |
|---|---|
| Stolen bearer | digest-only storage, owner-only client file, bounded expiry, atomic rotation, immediate connection/membership revocation, rate limits |
| Actor impersonation | principal and authorship resolved from the bearer; author IDs are never accepted from tool arguments |
| Cross-Project discovery | credential-bound Project, ordinary membership and policy checks, opaque cursors bound to connection and Project, common not-found responses |
| Replay or duplicate writes | stable client request IDs, request hashes, existing idempotency reservations, optimistic versions, authoritative conflict responses |
| Prompt or tool output injection | structured bounded schemas, inert Message AST, no raw HTML, no shell interpolation, no provider/runtime JSON persisted as authority |
| External runtime compromise | no database/GitHub/browser/provider secrets in MCP results; no host paths or repository contents; allowlisted tools; external filesystem remains outside Agent Workspace |
| Run-cost or profile theft | an external Actor cannot own or select a personal profile; Worker invocation creates a human-addressed approval and uses only the approving sponsor's eligible personal profile |
| Self-approval | separation of duty and `human_required` policy; an external Actor cannot approve its own Worker request or a human-only blocker |
| Event exfiltration | addressed event rows contain bounded IDs/kinds/summaries only; canonical reads reauthorize every resource; cursor cannot change Project scope |
| Disable race | credential, connection, Actor/membership, scope, archive, and resource state are revalidated transactionally at each write and before run admission |
| Noisy autonomous loop | per-credential budgets, bounded poll limit/long-poll duration, write idempotency, one active run per Thread, provider/Git concurrency limits |
| Runtime version drift | exact Hermes release and dependency lock are recorded; compatibility fixtures fail closed on protocol/tool-shape changes |

## Worker invocation boundary

An external agent may request bounded repository work but cannot silently
consume any human's model account. The request records the external Actor as
requester and requires an active human sponsor with an eligible personal
Execution Profile. Agent Workspace creates a human-required,
separation-of-duty approval addressed to that sponsor. Only the sponsor's
server-side approval may create the Agent Run; denial, expiry, disable, or
profile unavailability produces no run, workspace, Provider Session, or
credential exposure. The accepted run retains the external Actor as the
originating requester and the human as approving/sponsoring Actor.

## Exclusions

M9 does not host Hermes memory, schedules, gateway, model, terminal, skills,
or browser; implement A2A; accept inbound public webhooks; add MCP OAuth for
agents; share a human bearer; expose arbitrary SQL/Git/file tools; provide a
pooled provider account; or generalize across OpenClaw and other runtimes.
