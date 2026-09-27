# Working foundation for people and agents

September 27, 2026. Implement a useful account system now, preserving the IDs and organization boundaries needed for a later SaaS product.

## Build now

- Use WorkOS AuthKit for human sign-in. Keep the existing local account and actor ID; link from an authenticated account rather than matching names or emails. Retain local login during migration.
- Store usernames and application membership in PostgreSQL. WorkOS IDs are external identity mappings. Human usernames are globally unique; people and agents share a unique handle namespace inside each organization. Display names are not identity keys.
- Give humans and agents canonical organization profiles. Keep existing project profile fields as compatibility copies. Preserve all messages, DMs, and actor IDs.
- Let project administrators select an existing person or agent from their organization, add it with a role, and remove it without deleting its account.
- Let an agent's existing connection reach projects to which it has explicitly been added. Resolve project context per call, check current membership, and preserve private-conversation checks. Human grants remain restricted to their original project. Human mentions in project channels and threads create reply requests for the mentioned agents, so the existing crew bridge can answer them.
- Keep named agents separate from human logins and execution workers. Agents are first-class members with their own actor, handle, roles, credentials and authored messages; they do not need fake human emails or passwords.
- Add account settings for profile information and linking WorkOS. New WorkOS identities choose a unique username and get an isolated workspace. Existing project invitations can then add them as existing users.

## Storage and access rules

Use additive migrations for organization profiles, WorkOS identity mappings and short-lived login attempts. Continue using existing users, actors, organizations, memberships and sessions. Enforce unique names in PostgreSQL. The server chooses authorship from authenticated identity, never from an untrusted request field.

Never merge existing actors by display name. Backfill from their Direct-space profile where available, otherwise their oldest project membership. Stop migration on ambiguous handle collisions instead of changing somebody's identity silently.

WorkOS credentials stay in protected server files. Login state is browser-bound, single-use and expiring. Provider tokens do not enter browser JavaScript, application records or logs. The browser receives the existing opaque HttpOnly application session. Local revocation remains authoritative. WorkOS sessions are checked against the provider at most every five minutes and local sessions last at most 24 hours. Sign in again after expiry; this version does not persist provider refresh tokens.

Adding an agent grants access through its existing organization connections only to that project's current membership. Removing membership must stop reads, writes, inbox delivery and new replies there; its DM and other projects remain available. Every conversation query uses `conversation_visible_to`.

## WorkOS boundary

AuthKit owns human authentication. Workspace owns accounts, handles, project permissions and agent identity. Named agents can use WorkOS access tokens, existing MCP OAuth, or their bootstrap bearer credential. All three resolve to the same local account.

WorkOS has two separate agent features. Agent Auth blueprints mint short-lived tokens for agents operated by the application. Agent Registration and `auth.md` let external agents register and complete a user claim. The production dashboard exposes blueprint configuration, but that alone does not confirm the free production allowance or validate an integration.

Implement both issuer adapters. Verify the WorkOS signature, issuer, audience, expiry, agent claims and live revocation before resolving a local grant. Store WorkOS organization mappings and blueprint/registration bindings in PostgreSQL, referencing the existing agent connection and revocable credential. Intersect token permissions with the connection scopes. A token never creates project membership or borrows human authorship.

Agents settings enables an autonomous blueprint for an existing named agent. The runtime exchanges its bootstrap credential at `/api/v1/agents/token` for a five-minute access token and rotating refresh token. Sessions last at most 24 hours. Revoking the binding, disabling the connection, removing membership, or rotating the backing credential stops access. Reconnect the binding after credential rotation. Provider access and refresh tokens stay in the agent runtime, never application records.

External agents discover `/auth.md`, follow the WorkOS `service_auth` claim, and give the administrator their registration ID. Only the human who claimed that registration can attach it to a named Workspace agent. Unverified registrations have no access. Organization mappings are explicit and unique; no email or name auto-linking.

Enable Google OAuth in the production WorkOS environment with the application’s Google Cloud client, using only basic identity scopes. Add Google sign-in and Google account-link buttons while preserving the explicit account-link flow.

Production AuthKit requires billing information in the WorkOS dashboard, even under its free allowance. Use the included WorkOS hosted domain and email sender. Public signup is off by default through `AGENT_WORKSPACE_WORKOS_ALLOW_SIGNUP=false`. Do not silently use a staging environment as production identity infrastructure. Enable the production integration once configuration and a real account-link/sign-in check succeed.

## Ship

Implement the schema, shared member flow and account integration together, with focused PostgreSQL tests for identity linking, uniqueness, cross-organization access, removal and private conversations. Run the full repository gates on memory-capped Red Hat CI, merge the passing PR, back up the live database, deploy and check the real flows.

Success means: link the existing owner to WorkOS without losing access; add the existing Mara to a project; have her read and reply with the same identity; remove her and verify access stops there while her DM stays intact.

Billing, public signup rollout, enterprise SSO, extensive account lifecycle policy and multi-tenant code-execution hosting are later work. Keep organization IDs and provider mappings explicit so those additions do not require replacing the account model.

References: [AuthKit users](https://workos.com/docs/authkit/users-organizations), [authorization and PKCE](https://workos.com/docs/reference/authkit/authentication/get-authorization-url), [Agent Auth](https://workos.com/docs/authkit/agent-blueprints), [Agent Registration](https://workos.com/docs/authkit/agent-registration), [auth.md](https://workos.com/auth-md).
