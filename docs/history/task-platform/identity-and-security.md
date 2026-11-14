> **History.** Kept as a record of how Agent Workspace was built; don't follow it. Where it is going is in [`docs/plans/master/`](/docs/plans/master/README.md).

# Identity, Authentication, Tenancy, and Security

## Security posture

Reuse the accepted server-resolved Actor model and centralized action-on-
resource policy. A dedicated client is another presentation surface, not a
trust boundary and not a reason to weaken authorship, CSRF, Project isolation,
or external-agent scoping.

## Identity model

```text
User ── owns one human Actor
External Agent Connection ── authenticates one external_agent Actor
Execution Worker ── owns one execution_worker Actor
GitHub adapter ── acts as an integration Actor
Internal maintenance ── acts as a system Actor

Actor ── Organization Membership ── Organization
Actor ── Project Membership ─────── Project ── Task
```

Credentials authenticate clients; Actors authorize and attribute domain
actions. No Task request accepts `actorId`, `authorActorId`, or tenant ownership
as a writable authorship field.

## Tenancy

Organization is the tenant and ownership boundary. Project is the strong
resource scope. Every Task belongs to one Project; the Project belongs to one
Organization. Reads and writes validate both active Organization membership
and active Project membership where the accepted policy requires them.

Project-scoped database foreign keys remain defense in depth. Application
queries must include Project scope rather than fetching by UUID and checking
later. Cross-tenant missing and unauthorized records return non-disclosing
results.

### Personal spaces

Personal work uses a private Personal Organization and one private Personal
Project. The dedicated UI calls its default view Inbox. Do not introduce Tasks
owned directly by `users.id`, Tasks with nullable `project_id`, or a special
authorization path.

The initial owner bootstrap may create the Personal Project additively if one
does not exist. Identification requires a stable server-owned marker or
relation, not a mutable Project name. Exactly one Personal Project per Personal
Organization is the v1 invariant.

## Authentication options

### Browser

Retain the current private-host model:

- Argon2id local passwords
- opaque high-entropy session token; only its digest is stored
- session rotation on login and authentication-epoch revocation
- `SameSite=Strict` cookies
- `Secure` `__Host-` cookies in TLS deployments
- session cookie is `HttpOnly`; CSRF cookie is readable only for the exact
  double-submit header flow
- allowed-Origin validation and CSRF on every browser mutation
- immediate session/WebSocket revocation when membership or account state
  changes

Task-only mode must not add public signup. First owner remains host-bootstrapped;
additional users enter through single-use, owner-issued invitations.

### Local/API clients

Prefer OAuth authorization code with PKCE S256 and explicit consent. Access and
refresh tokens are hashed, bounded, rotatable, revocable, audience-bound, and
scoped. High-entropy direct bearers remain a private development or explicitly
managed integration path, not the default consumer API-key UX.

Token scopes are coarse gates such as `tasks:read`, `tasks:write`,
`tasks:comment`, and `events:read`. Ordinary Project membership and resource
policy still run; scopes never grant membership.

### External agents

Retain connection-scoped high-entropy bearers with digest-only storage, one
external-agent Actor, one Project, explicit connection status, immediate
rotation/revocation, bounded polling, and audit. A connector cannot self-enroll,
select another Actor, or obtain human/Worker credentials.

## Authorization actions

Extend the centralized typed policy only as implementation requires:

- `TaskView`, `TaskCreate`, `TaskEdit`, `TaskComment`, `TaskArchive`
- `TaskAssign`, if resource-specific assignment rules diverge from edit
- `TaskManageWorkflow`, `TaskManageLabels`, `TaskManageViews`
- `TaskBatchEdit`, `TaskImport`, `TaskExport`

Baseline roles remain Owner, Admin, Member, Viewer, and Worker. Owner/Admin/
Member can create and edit ordinary work; Viewer is read-only; Worker is limited
to immutable delegated scope. Actor kind alone grants nothing.

The server returns capabilities derived from policy for UI rendering. A client
must tolerate capability changes and server denial between read and mutation.

## Assignment and participation

Assignees are active human or external-agent Project members. Execution Workers
are bounded delegated capabilities and remain invalid assignees. Disabling an
Actor or membership prevents new reads/writes/assignments and marks historical
identity inactive without rewriting attribution.

Mentions in comments validate the mentioned Actor's active Project membership
at append time. Historical comments remain immutable after disable.

## Capability computation

Effective capability is the intersection of:

```text
deployment feature availability
∩ credential scopes
∩ active Actor status
∩ active Organization membership
∩ active Project membership and role
∩ resource-specific conditions
∩ delegated scope or separation-of-duty rules
```

Capabilities are recalculated server-side on every command. Cacheable read
representations may include them, but they are short-lived hints.

## Cross-tenant non-disclosure

- Resolve Task by `(project_id, task_id)` or authorized key, never global key.
- Do not expose whether an inaccessible Project, Actor, label, Task, or link
  exists.
- Search, counts, cursors, export metadata, event feeds, and error details are
  filtered before serialization.
- A cursor is bound to Actor/credential, Project or authorized aggregate, query
  shape, and contract version when replay could otherwise cross scope.
- Reauthorize WebSocket and polling connections on session, token, Actor, or
  membership revocation.

## CSRF and browser isolation

The dedicated client should be served from the same trusted origin as the API
in integrated and standalone modes. This preserves the current cookie/Origin
contract and avoids CORS credential complexity.

If a future separately hosted client origin is required, add it only through an
explicit allowlist, exact origin comparison, preflight policy, CSP review, and
CSRF threat model. Never use `Access-Control-Allow-Origin: *` with credentials.

## API tokens

User-created long-lived general API keys are not part of the first task product
release. Start with existing OAuth and managed scoped bearers. If personal API
tokens are later added, require:

- one-time plaintext display
- digest-only storage and safe prefix metadata
- explicit scopes and optional Project allowlist
- expiration, rotation, last-used metadata, and immediate revoke
- audit for create/rotate/revoke/use class
- rate limits separate from browser sessions
- no ability to mint broader credentials

## Data and payload security

- Bounded title, description, comment, label, query, batch, and export sizes
- Structured comment AST allowlist; no arbitrary HTML
- Escaped rendering and safe URL schemes
- No provider payloads, hidden prompts, secrets, filesystem paths, webhook
  bodies, or database diagnostics in Task records/API
- Audit payloads use allowlisted metadata, not entire requests
- Export omits secrets and inaccessible linked resources
- Logs identify request/event IDs but redact tokens, cookies, authorization
  headers, content where unnecessary, and all secret material

## Standalone-mode attack surface

Task-only mode must not initialize or expose:

- Codex App Server or provider processes
- Git/worktree operations
- GitHub webhooks, App credentials, or installation token minting
- repository registration/preparation/publish routes
- Worker invocation, approvals, or external execution profiles

The compiled binary may contain these modules, but route registration,
background workers, health dependencies, configuration, and capabilities must
fail closed according to mode.

## Future hosted multi-tenancy boundary

Public multi-tenant hosting remains a future security project. Before it can be
selected, require tenant-aware threat modeling, public identity provider and
account recovery, abuse/rate controls, database isolation evidence, background
job fairness, export/privacy policy, audit access, regional/backup policy, and
remote isolated execution. The task-only surface reduces execution risk but
does not by itself prove hostile-tenant isolation.

## Security acceptance tests

- Browser mutation fails without exact Origin/CSRF and succeeds with current
  session flow.
- Cross-Project UUID/key/search/cursor requests are non-disclosing.
- Viewer, disabled member, disabled Actor, revoked session/token, and insufficient
  scope fail across HTTP, WebSocket, MCP, batch, and export.
- Client-supplied authorship is rejected as an unknown field.
- Idempotent replay preserves original Actor and response.
- Capability hints never authorize a command after a role change.
- Task-only startup opens no provider, GitHub webhook, Git, or database-public
  listeners.
