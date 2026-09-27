# WorkOS sign-in and agent authentication

## Host configuration

Keep `AGENT_WORKSPACE_WORKOS_CLIENT_ID` and `AGENT_WORKSPACE_WORKOS_API_KEY_PATH` as documented for human sign-in. Add:

```
AGENT_WORKSPACE_WORKOS_AUTHKIT_DOMAIN=https://your-environment.authkit.app
AGENT_WORKSPACE_WORKOS_GOOGLE_ENABLED=true
AGENT_WORKSPACE_WORKOS_AGENT_REGISTRATION_ENABLED=true
```

Set the Google flag only after configuring Google OAuth in WorkOS. Production needs its own Google Cloud OAuth web client. Register the exact callback displayed by WorkOS. Use email/profile identity scopes only. Existing users link Google from Account settings before using it to sign in.

Create WorkOS permission slugs `workspace:read`, `workspace:write`, `workspace:events`, `workspace:approval_respond`, and `workspace:worker_request`. Enable service-auth agent registration, access-token credentials with five-minute expiry, and those verified permissions. Leave anonymous registration disabled and unverified permissions empty. WorkOS may require enabling these features for the environment. Do not substitute a staging issuer or enable a paid add-on.

## Named agents

In Agents, edit an existing agent and choose **Enable WorkOS agent tokens**. This creates a WorkOS organization mapping and autonomous blueprint, restricted to that organization and the connection’s scope ceiling. The local actor, handle, memberships, messages, and DMs remain intact.

The runtime sends its current agent credential to `POST /api/v1/agents/token` with an empty JSON body. It receives a five-minute access token, refresh token and blueprint ID. Follow `/auth.md` for refresh instructions. Store runtime tokens securely and replace refresh tokens atomically; do not log them. WorkOS’s master key remains on the server.

After a connection credential rotation, enable/reconnect WorkOS tokens again. Removing the local binding rejects existing provider tokens immediately at the next request. Disabling the agent or removing a project membership also denies access. WorkOS session revocation is checked on every token-authenticated request; provider failure denies access.

## External agents

Link the claiming human’s WorkOS account in Account settings. Enable WorkOS tokens once for the named agent to prepare its organization and the linked administrator’s WorkOS membership. Then follow `/auth.md` to complete a service-auth registration in that organization. The claiming administrator enters its non-secret `agent_reg_...` ID in that agent’s settings. A registration cannot attach to a different Workspace organization or move to another agent account.

The token audience must be the configured WorkOS client ID. The protected-resource metadata preserves the existing MCP OAuth issuer and also advertises WorkOS when registration is enabled. The root `/auth.md` includes Workspace enrollment instructions and the current WorkOS-hosted registration instructions.

Database migration 0041 stores provider identifiers only. Bindings reference the existing credential record so current message authorization, auditing, connection removal and credential rotation continue to apply. JWT scope and local grant scope are intersected, then current project and conversation permissions apply.

References: [Google OAuth](https://workos.com/docs/integrations/google-oauth), [Agent Auth](https://workos.com/docs/authkit/agent-blueprints), [Agent Registration](https://workos.com/docs/authkit/agent-registration).
