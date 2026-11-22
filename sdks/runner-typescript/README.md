# @agent-workspace/runner

A runner SDK for Agent Workspace in TypeScript, with no dependencies. It
hears reply requests on the event log (a stream, long-poll, or signed pushes
to your address), claims each one, gives your code the prompt and a few calls
to answer with, stops your work when a person presses Stop, and hands
follow-ups to the work already running in their thread.

Underneath is `WorkspaceChannel`, the reference channel adapter: everything a
chat integration needs, for when you want more than reply requests.

Node 22.18 or later runs `src/*.ts` as is. `npm run build` writes a plain
JavaScript copy to `dist/` for older setups.

## Use

```ts
import { Runner, runnerSettingsFromEnv } from '@agent-workspace/runner'

const settings = runnerSettingsFromEnv()
const runner = new Runner({
  ...settings,
  async onRequest(request) {
    // request.prompt is what was asked, without the leading @mention.
    // request.settings holds a final "Settings: model=… reasoning=… speed=…" line.
    await request.progress('Reading the repository')
    request.onFollowup(async (followup) => {
      // A person wrote again in the thread; answer it or pass.
      await followup.pass()
    })
    if (request.signal.aborted) return
    await request.reply(`You asked: ${request.prompt}`)
  },
})
runner.listen(8787) // POST /wake on 127.0.0.1; put your https proxy in front
```

Without a public address, read the stream instead of listening. It falls back
to long-polling where something on the way cuts streams, and picks up
requests already waiting first:

```ts
await runner.connect({ signal })
```

Give `cursorFile` to keep your place across restarts yourself; without one
the server keeps it for the connection.

## What a request gives you

| | |
|---|---|
| `prompt` | The message that asked, without leading mentions or the settings line. |
| `settings` | `{ model, reasoning, speed }` from a final `Settings:` line, when there is one. |
| `claim` | The full `reply_claim` result: `request`, `recentMessages`, `instructions`, `session`. |
| `sessionRef` | The session already bound to this conversation, to resume, or `null`. |
| `signal` | Aborts when a person presses Stop. Kill your child process on it. |
| `progress(status)` | One status line under the agent's name. Renews the 15-minute lease. |
| `interim(text)` | A message that keeps the claim (a plan, a pull request link). |
| `reply(text)` | The final answer. Closes the request. |
| `pass(reason?)` | Nothing to add (optional requests only). |
| `release(reason)` | You can't do it; people see the reason. |
| `bindSession(ref, { url?, state? })` | Names your session so the thread links to it and later claims carry it. |
| `runId` | The run a launch opened for this request, or `null`. The reply calls already move it. |
| `step(title)` | One step of the work on the run (nothing without one). |
| `result(kind, url, title?)` | Something the run produced, such as a `pull_request` (nothing without a run). |
| `onFollowup(listener)` | Follow-ups in this thread while you work. Each is a request of its own. |

If `onRequest` throws, the runner releases the request with a short reason.
Calls that the server refuses throw `ToolError` with the `outcome` (such as
`conflict` after Stop); `error.retryable` is true for
`temporarily_unavailable`.

## The channel

```ts
import { McpClient, WorkspaceChannel, fileCursor } from '@agent-workspace/runner'

const channel = new WorkspaceChannel({
  origin: 'https://workspace.example',
  token,
  tools: new McpClient({ url: 'https://workspace.example/api/v1/mcp', token }),
  cursor: fileCursor('.workspace-cursor'),
  onState: (state) => console.log(state), // connecting, streaming, polling, offline, closed
})
channel.on('message.mentioned', async (event) => {
  // Events carry ids only: read the text live with your own access.
  const conversationId = String(event.data.conversationId)
  await channel.send(conversationId, 'On it', { clientMessageId: `ack-${event.id}` })
})
channel.on('agent.*', (event) => console.log('paused or resumed:', event.type))
await channel.connect({ signal })
```

| | |
|---|---|
| `on(type, handler)` | A type, a family (`reply.*`) or every event (`*`). Each event reaches its handlers once, then the cursor moves past it. |
| `connect({ signal })` | Reads until the signal aborts. A refused credential throws `ChannelRefused`. |
| `pollOnce(signal?, waitSeconds?)` | One page, for your own loop. |
| `receivePush(secret, headers, body)` | A signed push to your address: answers `{ status }` and handles the event. |
| `send(conversationId, text, options?)` | A message; with `reply: { requestId, generation }` the answer to a reply request. `clientMessageId` comes back on `message.created`. |
| `edit`, `delete`, `react`, `typing` | `message_update`, `message_delete`, `message_react`, and `reply_progress_report` for the working line. |
| `download(fileId)`, `upload(...)` | Files through a short-lived link, and into a conversation. |

`verifyPush({ secret, headers, body })` checks Standard Webhooks signatures,
against several secrets during a rotation. The tests run against
`sdks/fixtures/delivery`.

## Lower-level pieces

- `McpClient({ url, token }).callTool(name, args)`: streamable HTTP with JSON
  or SSE answers.
- `ReplyTools`: `inboxList`, `claim`, `sessionBind`, `progress`, `send`,
  `pass`, `release`, and the run reports.
- `Deduper` for event ids; `parseSettings(text)` and `stripLeadingMentions(text)`.

## Secrets

Keep `AGENT_WORKSPACE_TOKEN` and `AGENT_WORKSPACE_WEBHOOK_SECRET` in a secret
store. Don't log them, and don't pass them on to the agents your runner
starts.

## Test

```sh
npm test   # node --test
```
