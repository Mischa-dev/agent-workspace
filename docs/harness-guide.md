# Harness guide

A harness is the program that does an agent's work: Codex, Claude Code, or
one of your own. There are three ways to plug one into Agent Workspace:

1. **A command-line harness.** The server runs your program once per turn in
   the run's worktree, and the two talk in JSON lines. This is the quickest
   way to try a harness on the host.
2. **A connected runner.** Your program runs wherever you like, gets wakeups,
   and answers with the MCP reply tools. The TypeScript and Python SDKs do
   the protocol for you.
3. **A built-in adapter.** A Rust module in the server, like the Codex and
   Claude Code adapters.

A command-line harness or a connected runner shows in Settings, the Launch
dialog and agent profiles once you register it with a manifest (see
[The manifest](#the-manifest)).

## 1. A command-line harness

Register it in **Settings > Workspace > Harnesses**. Give it a name, the
argument list to run, the names of the personal secrets it needs, and its
manifest. The program must be an absolute path inside a directory the
server's operator lists in `AGENT_WORKSPACE_COMMAND_HARNESS_DIRS`
(colon-separated). Anything outside those directories is refused, so a
workspace member can't run arbitrary programs on the host.

[`sdks/examples/script-harness.py`](../sdks/examples/script-harness.py) is a
working one in about 50 lines.

### How a turn runs

Each turn is one process. When it ends, the run commits what changed in the
worktree to the run's branch. A person opens the pull request from the run
card, which pushes the branch through the project's GitHub App. The harness
only edits files in its worktree: it never pushes, and it doesn't need GitHub
credentials.

**Arguments.** The registered argument list is expanded for each turn. An
element may contain these placeholders:

| Placeholder | Becomes |
|---|---|
| `{model}` | The model picked at launch. |
| `{reasoning}` | The reasoning level. |
| `{speed}` | The speed. |
| `{worktree}` | The worktree's absolute path. |
| `{option:NAME}` | The launch option `NAME` from the options schema. |

An element that was only a placeholder and expands to nothing is dropped, so
`["my-agent", "--model", "{model}"]` becomes `["my-agent", "--model"]` when
no model was picked. Put the flag and the value in one element
(`"--model={model}"`) if your program can't take a bare flag. The arguments
are passed as an array; no shell ever sees them.

**Directory and environment.** The working directory is the worktree. The
environment holds only:

```text
PATH=/usr/local/bin:/usr/bin:/bin
HOME=<the account's private home>
LANG=C.UTF-8
LC_ALL=C.UTF-8
AGENT_WORKSPACE_HARNESS_PROTOCOL=1
AGENT_WORKSPACE_SECRETS_FD=3        (only when secrets are handed over)
```

Nothing from the server's own environment reaches the program.

**Secrets.** File descriptor 3 carries one JSON object of name to value, then
end of file. Read it once, at the start. It holds the secrets the
registration names, taken from the account owner's personal secrets, plus
`API_KEY` when the account signed in with a key. Never print a secret, log
it, write it to a file in the worktree, or pass it to a child process in its
environment or arguments. If your agent needs one, hand it over the way the
server does: on a pipe or a file descriptor.

**stdin.** The server writes JSON lines. The first is the turn:

```json
{"type": "turn", "protocol": 1, "threadId": "…", "turnId": "…",
 "prompt": "Fix the flaky login test", "model": "my-model-large",
 "reasoning": "high", "speed": null, "options": {}, "resume": false}
```

`model`, `reasoning` and `speed` are `null` when none was picked. `resume` is
true when the thread has had turns before; keep your own session state in
`HOME` if you want to pick it up again.

Later lines answer your approvals and questions, or ask you to stop:

```json
{"type": "answer", "requestId": "a1", "decision": "approve"}
{"type": "answer", "requestId": "q1", "text": "Use the staging database"}
{"type": "stop"}
```

After `stop`, the process is killed a few seconds later. Stop your agent and
anything it started.

**stdout.** Your program writes JSON lines, at most 1 MiB each. Types the
server doesn't know are ignored, so a newer harness keeps working on an older
server.

| Line | What it shows |
|---|---|
| `{"type": "message", "text"}` | A message from the agent in the thread. |
| `{"type": "log", "text"}` | A line in the run's log. |
| `{"type": "command", "id", "command", "status": "started"}` | A command started. |
| `{"type": "command", "id", "status": "completed", "exitCode", "output"}` | The same command (by `id`) finished. |
| `{"type": "file", "path", "change": "added" \| "modified" \| "deleted"}` | A file changed, relative to the worktree. |
| `{"type": "tests", "command", "passed", "failed", "skipped"}` | A test run's counts. |
| `{"type": "approval", "id", "summary"}` | Asks a person to approve something. Wait for the `answer` with this `id` as `requestId`. |
| `{"type": "question", "id", "summary"}` | Asks a person a question. Wait for its `answer`. |
| `{"type": "usage", "model", "inputTokens", "outputTokens", "cachedInputTokens", "costUsd"?}` | Tokens used. Repeated lines add up. |
| `{"type": "limits", "windows": [{"key", "usedPercent", "resetsAt", "durationMinutes"}]}` | The account's rate-limit windows. |
| `{"type": "done", "status": "completed" \| "failed", "message"?}` | Ends the turn. |

A process that exits without a `done` line fails the turn. stderr is
discarded, so send anything people should see as `log` lines.

## 2. A connected runner

A connected runner is an agent on the Agents page with a runner attached:
Agent Workspace sends it signed wakeups, and it answers through the MCP reply
tools. [docs/agent-runner.md](agent-runner.md) is the protocol. The SDKs do
it for you:

- [`sdks/runner-typescript`](../sdks/runner-typescript) (`@agent-workspace/runner`,
  Node 22.18 or later, no dependencies)
- [`sdks/runner-python`](../sdks/runner-python) (`agent_workspace_runner`,
  Python 3.10 or later, standard library only)

Each one verifies the wakeup, claims the request, gives your code the prompt
and the launch settings, and offers `progress`, `interim`, `reply`, `pass`,
`release` and `bindSession`. It aborts your work's signal on Stop and hands
follow-ups to the work already running in their thread. Without a public URL,
it can poll the inbox instead of listening for wakeups.

[`sdks/examples/sdk-harness.ts`](../sdks/examples/sdk-harness.ts) takes a task
to a pull request in under 100 lines: it makes a branch in a checkout, runs
your agent, commits, pushes, opens the pull request with `gh pr create`, and
replies with the link.

```sh
AGENT_WORKSPACE_MCP_URL=… AGENT_WORKSPACE_TOKEN=… AGENT_WORKSPACE_WEBHOOK_SECRET=… \
AW_EXAMPLE_REPO=/srv/checkouts/app \
AW_EXAMPLE_AGENT='["/usr/local/bin/my-agent", "--yes"]' \
node sdks/examples/sdk-harness.ts
```

A connected runner does its own Git work, unlike a command-line harness.

To make it a choice in the Launch dialog, register it in **Settings >
Workspace > Harnesses** as a connected runner: pick its agent and give it a
manifest. A launch then asks the agent like a tag does, with the picked
settings on the message's last line:

```text
Settings: model=my-model-large reasoning=high speed=fast
```

The SDKs take that line off the prompt and hand it over as `settings`.

## 3. A built-in adapter

A built-in adapter lives in `apps/server/src/provider/` and implements two
traits from `provider/adapter.rs`:

- `HarnessAdapter`: its `manifest()`, `acquire_run` and `acquire_control` for
  an account's sessions, `health`, and shutdown.
- `HarnessSession`: start, resume, and interrupt threads and turns, plus
  `next_event`, `answer`, reading past turns, the account (sign in, sign
  out, rate limits, usage), and `list_models`.

Register it in `provider/registry.rs` and add one test that calls
`adapter_contract` from `provider/contract_tests.rs`. The contract checks the
manifest's shape, the run branch prefix, concurrency limits and model IDs,
and the turn lifecycle against a fake of your harness. `demo.rs` is the
smallest adapter to copy from. Keep the harness's own JSON inside the
module.

## The manifest

A registered harness describes itself with a manifest:

```json
{
  "name": "My agent",
  "description": "Our in-house coding agent.",
  "models": [
    {
      "id": "my-model-large",
      "displayName": "Large",
      "reasoningLevels": ["low", "medium", "high"],
      "prices": {
        "currency": "USD",
        "inputPerMillion": 3.0,
        "outputPerMillion": 15.0,
        "cachedInputPerMillion": 0.3
      }
    },
    { "id": "my-model-small", "displayName": "Small" }
  ],
  "options": {
    "type": "object",
    "properties": {
      "profile": { "type": "string", "enum": ["careful", "fast"] },
      "ticket": { "type": "string", "maxLength": 40 },
      "dryRun": { "type": "boolean" },
      "maxSteps": { "type": "integer", "minimum": 1, "maximum": 200 },
      "temperature": { "type": "number", "minimum": 0, "maximum": 1 }
    }
  },
  "features": { "approvals": true, "questions": true, "commands": true },
  "maxConcurrentRuns": 2,
  "apiKey": true
}
```

| Field | What it is |
|---|---|
| `name`, `description` | What Settings and the Launch dialog show. |
| `models` | The model picker. `id` is what `{model}` and `model=` carry. `reasoningLevels` and `prices` are optional. Prices are per million tokens. |
| `options` | Extra launch options, as a JSON Schema object. Only this subset is understood: `string` (with `enum` or `maxLength`), `boolean`, and `integer` or `number` (with `minimum` and `maximum`). A command-line harness reads them with `{option:NAME}` or from the turn's `options`. |
| `features` | `approvals` and `questions`: the harness may ask, so the run card shows the prompts. `commands`: it reports commands. |
| `maxConcurrentRuns` | How many runs an account may have going at once. |
| `apiKey` | True when an account can sign in with an API key. The key reaches a command-line harness as `API_KEY` on fd 3. |

## Tokens and cost

Every `usage` event (or a built-in adapter's token usage) is recorded with
the run: tokens in, out and cached. The cost is priced from the workspace's
model catalog, which starts from the manifest's `prices` and which an
administrator can edit. When the catalog has no price for the model, the
harness's own `costUsd` is used instead. Runs total up per project, account
and day.

## Secrets

- Never put a key or token in a child process's environment or arguments.
  Every child inherits the environment, and diagnostics print both.
- Never write one to logs, `log` or `message` lines, run events, test
  fixtures or the repository.
- A command-line harness gets its secrets once, on fd 3. A connected runner
  keeps its token and webhook secret in its own secret store, and doesn't
  pass them on to the agent it starts.
- Test against fake harnesses and fake servers, never with a real login or
  API key.
