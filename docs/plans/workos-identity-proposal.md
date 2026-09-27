# Working foundation for people and agents

September 27, 2026. Implement a useful account system now, preserving the IDs and organization boundaries needed for a later SaaS product.

## Build now

- Use WorkOS AuthKit for human sign-in. Keep the existing local account and actor ID; link from an authenticated account rather than matching names or emails. Retain local login during migration.
- Store usernames and application membership in PostgreSQL. WorkOS IDs are external identity mappings. Human usernames are globally unique; people and agents share a unique handle namespace inside each organization. Display names are not identity keys.
- Give humans and agents canonical organization profiles. Keep existing project profile fields as compatibility copies. Preserve all messages, DMs, and actor IDs.
- Let project administrators select an existing person or agent from their organization, add it with a role, and remove it without deleting its account.
- Let an agent's existing connection reach projects to which it has explicitly been added. Resolve project context per call, check current membership, and preserve private-conversation checks. Human grants remain restricted to their original project.
- Keep named agents separate from human logins and execution workers. Agents are first-class members with their own actor, handle, roles, credentials and authored messages; they do not need fake human emails or passwords.
- Add account settings for profile information and linking WorkOS. New WorkOS identities choose a unique username and get an isolated workspace. Existing project invitations can then add them as existing users.

## Storage and access rules

Use additive migrations for organization profiles, WorkOS identity mappings and short-lived login attempts. Continue using existing users, actors, organizations, memberships and sessions. Enforce unique names in PostgreSQL. The server chooses authorship from authenticated identity, never from an untrusted request field.

Never merge existing actors by display name. Backfill from their Direct-space profile where available, otherwise their oldest project membership. Stop migration on ambiguous handle collisions instead of changing somebody's identity silently.

WorkOS credentials stay in protected server files. Login state is browser-bound, single-use and expiring. Provider tokens do not enter browser JavaScript, application records or logs. The browser receives the existing opaque HttpOnly application session. Local revocation remains authoritative. WorkOS sessions are checked against the provider at most every five minutes and local sessions last at most 24 hours. Sign in again after expiry; this version does not persist provider refresh tokens.

Adding an agent grants access through its existing organization connections only to that project's current membership. Removing membership must stop reads, writes, inbox delivery and new replies there; its DM and other projects remain available. Every conversation query uses `conversation_visible_to`.

## WorkOS boundary

AuthKit owns human authentication. Workspace owns accounts, handles, project permissions and agent identity. WorkOS Agent Auth is an optional future credential issuer when access and pricing are confirmed. Standard MCP and existing agent credentials keep working while that product is unavailable.

Production AuthKit requires billing information in the WorkOS dashboard, even under its free allowance. Use the included WorkOS hosted domain and email sender. Public signup is off by default through `AGENT_WORKSPACE_WORKOS_ALLOW_SIGNUP=false`. Do not silently use a staging environment as production identity infrastructure. Enable the production integration once configuration and a real account-link/sign-in check succeed.

## Ship

Implement the schema, shared member flow and account integration together, with focused PostgreSQL tests for identity linking, uniqueness, cross-organization access, removal and private conversations. Run the full repository gates on memory-capped Red Hat CI, merge the passing PR, back up the live database, deploy and check the real flows.

Success means: link the existing owner to WorkOS without losing access; add the existing Mara to a project; have her read and reply with the same identity; remove her and verify access stops there while her DM stays intact.

Billing, public signup rollout, enterprise SSO, extensive account lifecycle policy and multi-tenant code-execution hosting are later work. Keep organization IDs and provider mappings explicit so those additions do not require replacing the account model.

References: [AuthKit users](https://workos.com/docs/authkit/users-organizations), [authorization and PKCE](https://workos.com/docs/reference/authkit/authentication/get-authorization-url), [Agent Auth](https://workos.com/docs/authkit/agent-blueprints).
