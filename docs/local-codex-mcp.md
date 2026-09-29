# Connect local Codex to Agent Workspace

This runbook creates a separate disposable Codex `0.146.0` profile. It does
not change the retained hosted Execution Profile, ordinary global Codex
configuration, system Node, Tailscale, or any public listener.

## 1. Forward the private application listener

Keep this process running on the Mac:

```bash
ssh -N -L 3100:127.0.0.1:3100 bot@redhat
```

In another terminal, verify both application and OAuth discovery through the
tunnel:

```bash
curl --fail http://127.0.0.1:3100/health/ready
curl --fail http://127.0.0.1:3100/.well-known/oauth-protected-resource/api/v1/mcp
```

Tunnel loss makes MCP unavailable but does not invalidate a bearer credential
or OAuth grant. Restart the forward, then restart the local Codex process so it
renegotiates the MCP session.

## 2. Create an isolated client profile

Set `CODEX_BIN` to the pinned executable without replacing the ordinary
`codex` command. Keep this profile outside the repository:

```bash
export CODEX_BIN='<path-to-codex-0.146.0>'
export CODEX_HOME="$HOME/.local/share/agent-workspace/codex-profiles/m6-local"
install -d -m 700 "$CODEX_HOME"
"$CODEX_BIN" --version
```

Stop unless the last command prints `codex-cli 0.146.0`.

## 3A. OAuth setup (recommended)

Create `$CODEX_HOME/config.toml` with mode `0600`:

```toml
[mcp_servers.agent_workspace]
url = "http://127.0.0.1:3100/api/v1/mcp"
oauth_resource = "http://127.0.0.1:3100/api/v1/mcp"
default_tools_approval_mode = "writes"
required = false
```

Start the authorization ceremony:

```bash
"$CODEX_BIN" mcp login agent_workspace --scopes read,write
```

Codex dynamically registers a public client and opens the Agent Workspace
consent page. Sign in to Agent Workspace if necessary, confirm the named Codex
client, select the intended Project, inspect the scopes, and choose Authorize.
The callback stays on a randomly selected loopback port. Agent Workspace never
issues a client secret.

Verify with:

```bash
"$CODEX_BIN" mcp list
"$CODEX_BIN" mcp get agent_workspace
```

Then start Codex in the trusted linked repository. Use `/mcp` to confirm that
`agent_workspace` negotiated successfully and that all reads and writes are
visible. After this succeeds, changing `required = false` to `required = true`
is appropriate for this disposable profile: startup will then fail instead of
silently continuing without Project context.

OAuth cleanup:

```bash
"$CODEX_BIN" mcp logout agent_workspace
"$CODEX_BIN" mcp remove agent_workspace
```

Logout revokes the stored grant through the authorization server. If a local
credential file remains in the disposable profile, remove the complete
`$CODEX_HOME` directory only after confirming no other profile data is there.

## 3B. Development bearer fallback

Use this only for isolated development evidence. Obtain the immutable Project
ID, human Actor ID, and later credential ID from safe seed or list output. On
the Mac, capture the one-time response directly into an owner-only file so the
token is not printed or typed into shell history:

```bash
umask 077
ssh bot@redhat "sudo -n /opt/agent-workspace/current/bin/agent-workspace-service-command mcp-credential-create --project-id <project-uuid> --actor-id <actor-uuid> --client-name codex-m6-local --scopes read,write --expires-days 1" > "$CODEX_HOME/mcp-credential.json"
export AGENT_WORKSPACE_MCP_TOKEN="$(jq -er .token < "$CODEX_HOME/mcp-credential.json")"
```

Use this profile entry instead of the OAuth entry:

```toml
[mcp_servers.agent_workspace]
url = "http://127.0.0.1:3100/api/v1/mcp"
bearer_token_env_var = "AGENT_WORKSPACE_MCP_TOKEN"
default_tools_approval_mode = "writes"
required = false
```

Verify through `codex mcp list`, `codex mcp get agent_workspace`, and `/mcp`,
then set `required = true` if desired. `default_tools_approval_mode = "writes"`
keeps read tools available while routing write tools through explicit user
approval. Do not change it to `approve` for acceptance evidence.

List only safe metadata if one-time creation output is uncertain:

```bash
ssh bot@redhat "sudo -n /opt/agent-workspace/current/bin/agent-workspace-service-command mcp-credential-list --project-id <project-uuid>"
```

Revoke without putting the token on the command line:

```bash
ssh bot@redhat "sudo -n /opt/agent-workspace/current/bin/agent-workspace-service-command mcp-credential-revoke --credential-id <credential-uuid> --expected-version <version>"
unset AGENT_WORKSPACE_MCP_TOKEN
```

Delete the owner-only JSON file after revocation. If output was lost before the
token was safely captured, list metadata and use `mcp-credential-replace` with
the exact credential ID and version; replacement atomically revokes the old
credential and prints the new token once.

## Behavior and diagnostics

The credential resolves one human Actor and one Project on every call. Reads
and writes still pass ordinary membership, archive, resource, and scope policy.
Clients cannot submit authorship. Writes use optimistic versions and stable
request IDs; on conflict, refetch, inspect the current projection and changed
fields, and retry only when the returned category permits it. MCP never creates
an Agent Workspace Thread or hosted run.

- `401` at `/api/v1/mcp`: log in again, restore the client environment, or
  inspect expiry/revocation with safe metadata. Browser cookies do not work.
- Startup timeout: confirm the SSH process, `/health/ready`, and discovery URLs;
  leave `required = false` until the connection is stable.
- Missing writes: confirm `read write` scope and inspect `codex mcp get` plus
  `/mcp`. The `writes` approval mode should prompt rather than hide them.
- Version conflict: call `task_open`, preserve
  the current version, and make an informed retry with a new request ID unless
  replaying the exact previously completed payload.
- Permission or archive error: do not retry blindly. Restore membership or the
  resource through the web application, then refetch.

Never commit the profile, token JSON, OAuth credential store, browser cookies,
authorization codes, callback URLs, or command output containing credentials.
