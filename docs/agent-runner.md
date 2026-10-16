# Agent runner contract

A runner is the program that turns "someone tagged @claude" into work: it
hears a wakeup, starts or resumes a Claude Code, Codex or other session, and
answers through Agent Workspace's MCP tools. This page is the protocol a
runner implements. Agent Workspace never runs the runner; it only talks to it.
The same protocol serves every runner, so `@codex` and `@claude` behave the
same way.

The server serves this page at `{origin}/agent-runner.md`, with `{origin}`
filled in.

## Setup

On the Agents page, **Add a runner** and pick Claude Code, Codex or Custom.
Give it a handle and the URL your runner listens on. You get, once:

| Setting | What it is |
|---|---|
| `AGENT_WORKSPACE_MCP_URL` | `{origin}/api/v1/mcp`, the MCP endpoint (streamable HTTP). |
| `AGENT_WORKSPACE_TOKEN` | The agent's bearer token for that endpoint. Keep it in a secret store. |
| `AGENT_WORKSPACE_WEBHOOK_SECRET` | Signs every wakeup. Keep it secret too. |
| `AGENT_WORKSPACE_HANDLE` | The `@handle` people tag. |
| `AGENT_WORKSPACE_RUNNER` | `claude_code`, `codex` or `custom`, for your runner's own use. |

A runner without a public URL can skip the webhook and poll `inbox_list`
instead (every 30 to 60 seconds is plenty). Everything below still applies;
only the wakeup is missing.

## 1. Verify the wakeup

Agent Workspace sends `POST` to your URL with a JSON body and these headers:

- `x-workspace-delivery`: the delivery ID. Retries reuse it, so de-duplicate
  on it.
- `x-workspace-event`: the event, the same as the body's `event`.
- `x-workspace-timestamp`: Unix seconds.
- `x-workspace-signature`: `sha256=` and the hex HMAC-SHA256 of
  `"{timestamp}.{body}"` with the webhook secret.

Reject a signature that doesn't match (compare in constant time) or a
timestamp more than five minutes old. Answer `2xx` quickly and do the work
afterwards: a delivery that times out (8 seconds) or fails is retried with
backoff, up to 8 times.

The body never contains message text. To read what was asked, claim the
request; every read is checked against what the agent can see.

```json
{
  "event": "reply.requested",
  "deliveryId": "0199…",
  "agentId": "0199…",
  "projectId": "0199…",
  "replyRequestId": "0199…",
  "conversationId": "0199…",
  "threadId": null,
  "messageId": "0199…",
  "generation": 0,
  "followsRequestId": null,
  "sessionRef": null
}
```

| `event` | What happened | What to do |
|---|---|---|
| `reply.requested` | Someone asked the agent: a tag, a DM, a group message. | Start a session and claim the request. |
| `reply.followup` | A person wrote again in a thread where the agent holds a claim. `followsRequestId` is that claim. | Feed it into the session already working on `followsRequestId`, then claim it when you get to it. |
| `reply.cancelled` | A person pressed Stop. | Stop the session working on `replyRequestId` now. Don't reply to it. |

`threadId` is set once a channel tag has its thread. `sessionRef` is reserved
for the session binding that comes with agent sessions; it is `null` for now.

Webhooks set up before typed wakeups receive `{"event": "inbox.available",
"deliveryId", "agentId", "replyRequestId"}` instead, and no stop events. That
format stays for one release. Saving the webhook again on the Agents page
issues a new secret and switches it to typed wakeups.

## 2. Claim

Call `reply_claim` with the `requestId`. It returns:

- `request`: the request, with `generation` (pass it to every later call),
  `replyConversationId` (where your answers go), `threadId`,
  `followsRequestId` and `required`.
- `recentMessages`: the conversation as context, oldest first. For a thread,
  the message it started from comes first.
- `instructions`: what's expected for this kind of request.

Claiming starts a 15-minute lease. Report progress, post an interim message
or claim again to renew it. A lease that lapses becomes "didn't answer in
time", and a person has to ask again.

A tag in a project channel is answered in a thread started from the tagged
message. The server starts it when you claim, so `replyConversationId` is the
thread. Keep one session per thread: key your sessions on
`replyConversationId`, and route follow-ups with `followsRequestId`.

## 3. Work in the open

- `reply_progress(requestId, generation, status)`: one line (up to 200
  characters) shown under the agent's name, such as "Running the server
  tests". Send one when the work changes phase, and at least every 15
  minutes.
- `reply_send(requestId, generation, text, final: false)`: an interim message
  (a plan, a question, a pull request link). The claim stays yours.
- `reply_send(requestId, generation, text)`: the final answer. It closes the
  request.
- `reply_pass` (optional requests only, such as follow-ups and group
  messages): nothing to add. Posts nothing.
- `reply_release(requestId, generation, reason)`: you can't do it. People see
  the reason and can ask again.

Every call is safe to retry: `reply_send` derives its retry key from the
request, the generation and (for interim messages) the text.

## 4. Follow-ups

When a person writes again in a thread where the agent was tagged, worked or
replied recently, the agent is asked again without a new mention. A follow-up
never replaces a claim you hold: it waits as its own request, with
`followsRequestId` naming your claim, and the wakeup is `reply.followup`.
Read it into the running session (claim it, or call `conversation_open` on
the thread), and answer it with `reply_send` or `reply_pass` once you have.

## 5. Stop

When a person presses Stop, the request and the follow-ups queued behind it
become `cancelled`, and a `reply.cancelled` wakeup goes out at once. Every
later `reply_*` call on them returns `conflict` with a message saying it was
stopped and by whom. End the session's work and post nothing more on it.
A person can ask again, which sends a fresh `reply.requested`.

## Outcomes

Every reply tool returns `{ "outcome": … }`:

| `outcome` | Meaning |
|---|---|
| `ok` | Done; `value` holds the result. |
| `conflict` | The request can't take this call any more; `message` says why (answered, expired, replaced by a newer message, removed, stopped). Don't retry it. |
| `not_found` | Not a request for this agent. |
| `invalid_request` | `field` names the bad argument. |
| `permission_denied` | The token lacks the scope, or the agent lost access. |
| `temporarily_unavailable` | Retry with backoff. |

## A minimal runner

```text
on POST /wake:
    verify signature and timestamp, else 401
    if seen(deliveryId): return 204
    queue(body); return 204

worker:
    for wakeup in queue:
        match wakeup.event:
            "reply.cancelled": sessions.stop(wakeup.replyRequestId)
            "reply.followup":  sessions.for_request(wakeup.followsRequestId).tell(wakeup)
            "reply.requested":
                claim = mcp.reply_claim(wakeup.replyRequestId)
                session = sessions.resume_or_start(claim.request.replyConversationId)
                session.run(claim, progress = mcp.reply_progress,
                            interim = mcp.reply_send(final: false),
                            answer = mcp.reply_send)
```
