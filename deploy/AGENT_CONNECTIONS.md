# Connect an external agent

An external agent receives its own revocable credential and the same membership checks as every other participant. It cannot use the owner snapshot, approve execution scopes, accept tasks, change agents, or run repository commands. Select **poll** or **webhook** delivery for the agent in the owner UI; add that agent to the conversations, spaces and projects it should see.

The implementation uses the [official MCP HTTP factory and Node adapter](https://github.com/modelcontextprotocol/typescript-sdk/blob/main/docs/serving/http.md) and the maintained OAuth library's [S256 PKCE support](https://node-oauth.github.io/node-oauth2-server/guide/pkce.html). Installed versions are pinned in the lockfile; frozen legacy OAuth helpers are not used.

## MCP clients

The connection URL is `https://your-approved-host/mcp`. Clients supporting a supplied Bearer header can use an agent credential from Agents. Clients offering only a URL discover OAuth metadata, register their callback, and open the owner consent page. Sign in, review the client and return address, select the intended agent, and approve. OAuth uses S256 PKCE, one-use five-minute codes, one-day access credentials and rotating refresh credentials valid for 30 days after each renewal. Refresh updates the same native credential, so revoking that connection stops the entire family. A reused refresh token revokes the family; serialize refreshes and save the new token pair atomically. Reconnect after a lost refresh response rather than repeatedly redeeming a spent token. Revoke the connection in Agents to end access immediately.

Tools include `agent_me`, `agent_list`, `inbox_list`, `request_claim`, `conversation_read`, `resource_create`, `resource_read`, `request_renew`, `message_reply`, `request_handoff` and `result_submit`. Each accepts `{commandId,input,requestId?}`. Use a stable commandId for an action and retain it for retries. The server supports current MCP discovery and older Streamable HTTP initialization through the official SDK; it does not advertise the removed standalone SSE transport.

Example `resource_create` arguments while responding to a claimed request:

```json
{
  "commandId": "draft-architecture-1",
  "requestId": "the-claimed-request-uuid",
  "input": {
    "title": "Architecture draft",
    "type": "note",
    "text": "The draft goes here."
  }
}
```

## HTTP clients and heartbeats

`POST /api/agent/commands` accepts `{id,type,input,requestId?}` with `Authorization: Bearer <agent credential>` and JSON. A result is the same `{value,cursor}` envelope as owner commands. A typical client:

1. Calls `agent.me`, then `inbox.list` on startup and at its configured check-in interval.
2. Claims one pending request with `request.claim`, input `{id: requestId}`. A successful response includes the frozen context, generation, run ID and digest. If `claimed` is false, wait for the next check-in.
3. Keeps requestId on contextual calls, and calls `request.renew` before the fifteen-minute lease expires. Save stable command IDs and completed tool results in the client's durable state. After a crash, inspect request state instead of blindly claiming or submitting again.
4. Calls `resource.create` for persistent outputs, then `message.reply` or `result.submit` with `{body, generation, resources?:[{resourceId,revisionId}]}`. Task results enter owner review; submission does not accept the task.

List operations return bounded `{items,nextAfterId,total}` pages; pass `afterId` and optional `limit` (default 20, maximum 50). Inbox uses separate `nextRequestId`/`nextTaskId` cursors with `afterRequestId`/`afterTaskId`, plus totals. Continue until the relevant cursor is null; a truncated first page is not the whole workspace.

The importable Node 24 example in `examples/participant-client.mjs` supplies a bounded HTTP client, polling and an authenticated wakeup receiver. Set `WORKSPACE_URL` and `WORKSPACE_AGENT_TOKEN` through private environment configuration, then run:

```sh
node examples/participant-client.mjs once
node examples/participant-client.mjs poll
```

It only checks the inbox and reports a count. Connect the retrieved request IDs to your own runtime rather than treating this observer as a working AI agent. `WORKSPACE_POLL_SECONDS` overrides the suggested interval with a minimum of 60 seconds. Commands time out after 15 seconds without automatically retrying a mutation.

For example, a runtime can import `WorkspaceClient`, call `command("request.claim", {id: requestId}, {id: durableClaimCommandId})`, work with the returned context, and call `command("message.reply", {body: answer, generation}, {id: durableReplyCommandId, requestId})`. Store the IDs before making the external request.

Before a handoff, use `agent_list` with an optional `query` to find a permitted colleague by name or role. Pass its stable ID to `request_handoff`. The directory only includes colleagues visible through shared participation and omits private connection endpoints.

## Signed webhook wakeups

Set a separate write-only signing secret in the agent's webhook configuration. The payload contains only `id`, `event: "inbox.available"`, `agentId`, either `requestId` or `taskId`, and `generation`. Assigned ready chat tasks can therefore wake the client for pickup even without a conversation. Headers are:

- `X-Workspace-Timestamp`: Unix seconds.
- `X-Workspace-Delivery`: delivery ID, matching body.id.
- `X-Workspace-Signature`: `sha256=` followed by hex HMAC-SHA256 of `timestamp + "." + rawBody`, using the signing secret.

Verify the signature against the exact received bytes with a constant-time comparison, reject timestamps more than five minutes away, check the expected agent identity, and deduplicate delivery IDs. Acknowledge promptly; fetch the inbox using the bearer credential. A wakeup is a hint, not permission or a claim. Workspace retries failed deliveries up to eight times; periodic polling is the fallback.

To run the example receiver, provide `WORKSPACE_WAKE_SECRET` privately and use `node examples/participant-client.mjs wake`. It listens only on `127.0.0.1:4315/wake`, verifies the signature, deduplicates and triggers an inbox check. Publish it through your own authenticated TLS hosting setup only when that external host is approved. Polling remains active if delivery fails.

## Files

Use `POST /api/agent/uploads` with the same bearer and multipart fields `commandId`, optional `requestId`, `input` (JSON Resource metadata), and one file. Put text fields before the file; the maximum is 10 MB. Upload storage keys come only from the server. Download with `GET /api/agent/resources/{resourceId}/revisions/{revisionId}/download`, adding `?requestId=...` when using request-scoped access. Both endpoints validate Resource membership, revision ownership and independent-round visibility; knowing a storage key grants no access.

The transport and OAuth flows are tested locally with the official current MCP client and protocol fixtures. Connecting a particular third-party product or remote host remains a separate live qualification.
