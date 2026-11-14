# agent-workspace-runner

A runner SDK for Agent Workspace in Python 3.10 or later, standard library
only. It does the protocol in [docs/agent-runner.md](../../docs/agent-runner.md)
for you: checks the wakeup's signature, claims the request, gives your code
the prompt and a few calls to answer with, sets `stopped` when a person
presses Stop, and hands follow-ups to the work already running in their
thread.

## Use

```python
from agent_workspace_runner import Runner, runner_settings_from_env


def on_request(request):
    # request.prompt is what was asked, without the leading @mention.
    # request.settings holds a final "Settings: model=… reasoning=… speed=…" line.
    request.progress("Reading the repository")
    request.on_followup(lambda followup: followup.pass_())
    if request.stopped.is_set():
        return
    request.reply(f"You asked: {request.prompt}")


runner = Runner(on_request=on_request, **runner_settings_from_env())
server = runner.listen(8787)  # POST /wake on 127.0.0.1; put your https proxy in front
server.serve_forever()  # or keep the main thread busy some other way
```

Each wakeup runs `on_request` on its own thread. Without a public URL, poll
instead of listening:

```python
runner.poll(interval_seconds=30)
```

## What a request gives you

| | |
|---|---|
| `prompt` | The message that asked, without leading mentions or the settings line. |
| `settings` | `model`, `reasoning`, `speed` from a final `Settings:` line, when there is one. |
| `claim` | The full `reply_claim` result: `request`, `recentMessages`, `instructions`, `session`. |
| `session_ref` | The session already bound to this conversation, to resume, or `None`. |
| `stopped` | A `threading.Event` set when a person presses Stop. Kill your child process on it. |
| `progress(status)` | One status line under the agent's name. Renews the 15-minute lease. |
| `interim(text)` | A message that keeps the claim (a plan, a pull request link). |
| `reply(text)` | The final answer. Closes the request. |
| `pass_(reason=None)` | Nothing to add (optional requests only). |
| `release(reason)` | You can't do it; people see the reason. |
| `bind_session(ref, url=None, state=None)` | Names your session so the thread links to it and later wakeups carry it. |
| `run_id` | The run a launch opened for this request, or `None`. The reply calls already move it. |
| `step(title)` | One step of the work on the run (nothing without one). |
| `result(kind, url, title=None)` | Something the run produced, such as a `pull_request` (nothing without a run). |
| `on_followup(listener)` | Follow-ups in this thread while you work. Each is a request of its own. |

If `on_request` raises, the runner releases the request with a short reason.
Calls the server refuses raise `ToolError` with the `outcome` (such as
`conflict` after Stop); `error.retryable` is true for
`temporarily_unavailable`.

The lower-level pieces are there too: `verify_wakeup`, `sign_wakeup`,
`DeliveryDeduper`, `McpClient(url, token).call_tool(name, arguments)`,
`ReplyTools`, `parse_settings` and `strip_leading_mentions`.

## Secrets

Keep `AGENT_WORKSPACE_TOKEN` and `AGENT_WORKSPACE_WEBHOOK_SECRET` in a secret
store. Don't log them, and don't pass them on to the agents your runner
starts.

## Test

```sh
python3 -m unittest
```
