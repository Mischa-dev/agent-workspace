# Agent OS connector (bridged)

`aw-agentos-connector` runs Agent OS agents as a host on Agent Workspace. It
speaks only the public host protocol (`crates/host-protocol`) and calls
Agent OS through its controller command, so neither side needs the other's
internals.

It runs beside an Agent OS installation and dials out to the workspace:

1. In the workspace, link a host (Settings › Hosts) and copy the one-time
   code.
2. Write a configuration file from `examples/connector.example.json`: the
   workspace, a state directory, the Agent OS context and controller
   command, and the agents it serves (the reference each is bound with, its
   Agent OS id and its name).
3. `aw-agentos-connector --config connector.json enrol <code>` keeps the
   host's credential in the state directory, readable by its owner only.
4. `aw-agentos-connector --config connector.json check` proves both sides
   answer.
5. `aw-agentos-connector --config connector.json run`, under a service
   manager. Bind each agent to the host in the workspace.

## What it does

- **A start** (someone mentions or launches the agent) becomes one Agent OS
  `task-submit` against the deployment's current source revision, with the
  idempotency key `workspace-{run}`. The call is written to a receipt before
  and after. If Agent OS's answer is lost, or it took the task without a
  runtime run, the start is not acknowledged: the workspace offers it again
  and the very same submission is sent again, which Agent OS takes once.
- **While it runs** the task is looked at every `pollSeconds`
  (`task-inspect`), and the run shows as working.
- **When it ends** its output becomes the agent's message on the run. An
  answer of `{"decision": "pass"}` (or `pass`) means it has nothing to add.
  A failed or interrupted task fails the run with the reason, and one still
  running after `taskDeadlineSeconds` is stopped.
- **Stop** is `task-cancel`.

- **Tasks started in Agent OS itself** (a schedule there, say) for a served
  agent are found every `adoptSeconds` through `operations.listTasks` and
  added to the workspace as runs of their agent, once each (the task id
  finds the same run again), then watched and reported like any start.
  Agent OS doesn't report a task's steps, so such a run says only that it is
  working until it ends.
- **Usage**: `operations.usage` gives the plan windows of the accounts Agent
  OS runs on; they show on the workspace's usage page.
- **Offers**: `offers` lists what the host offers to set up when someone adds
  an agent from it.
- **Agent actions**: with `operations.pause` and `operations.memory`, each
  agent gets "Pause" and "What it remembers" on its page.

Each of these is off until its operation is named in the configuration.
Steering a running task and answering its questions are not supported.

## Tests

`cargo test -p agent-workspace-agent-os-connector` runs it against a fake
controller: one task per start however often it is offered, a lost answer
kept and retried with the same submission, refusals remembered, answers,
passes and failures reported, Stop, and no installation's names in the
code.
