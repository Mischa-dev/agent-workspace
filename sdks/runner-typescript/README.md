# @agent-workspace/runner

A runner SDK for Agent Workspace in TypeScript, with no dependencies. It does
the protocol in [docs/agent-runner.md](../../docs/agent-runner.md) for you:
checks the wakeup's signature, claims the request, gives your code the
prompt and a few calls to answer with, stops your work when a person presses
Stop, and hands follow-ups to the work already running in their thread.

Node 22.18 or later runs `src/index.ts` as is. `npm run build` writes a plain
JavaScript copy to `dist/index.js` for older setups.

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

Without a public URL, poll instead of listening:

```ts
await runner.poll({ intervalMs: 30_000 })
```

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
| `bindSession(ref, { url?, state? })` | Names your session so the thread links to it and later wakeups carry it. |
| `runId` | The run a launch opened for this request, or `null`. The reply calls already move it. |
| `step(title)` | One step of the work on the run (nothing without one). |
| `result(kind, url, title?)` | Something the run produced, such as a `pull_request` (nothing without a run). |
| `onFollowup(listener)` | Follow-ups in this thread while you work. Each is a request of its own. |

If `onRequest` throws, the runner releases the request with a short reason.
Calls that the server refuses throw `ToolError` with the `outcome` (such as
`conflict` after Stop); `error.retryable` is true for
`temporarily_unavailable`.

## Lower-level pieces

- `verifyWakeup({ secret, headers, body })` and `signWakeup(secret, timestamp, body)`.
- `DeliveryDeduper` for delivery IDs.
- `McpClient({ url, token }).callTool(name, args)`: streamable HTTP with JSON
  or SSE answers.
- `ReplyTools`: `inboxList`, `claim`, `sessionBind`, `progress`, `send`,
  `pass`, `release`.
- `parseSettings(text)` and `stripLeadingMentions(text)`.

## Secrets

Keep `AGENT_WORKSPACE_TOKEN` and `AGENT_WORKSPACE_WEBHOOK_SECRET` in a secret
store. Don't log them, and don't pass them on to the agents your runner
starts.

## Test

```sh
npm test   # node --test
```
