# Writing a host

A host is a program that runs agents for a workspace. The workspace never
runs an agent itself: it sends the host commands, and the host reports back
on each run. Everything a host needs is in the public protocol crate,
`crates/host-protocol`; `examples/minimal-host` is a complete host in about
four hundred lines, and `crates/host-conformance` checks yours.

## 1. Enrol

Someone who may manage hosts makes a one-time code in the workspace's host
settings (or a person makes one for a host of their own). The host sends it
once:

```rust
let enrolled = HostClient::enrol(server, &EnrolRequest {
    code, host: HostSoftware { name, .. },
    protocol_versions: vec![PROTOCOL_VERSION.to_owned()], ..
}).await?;
let client = HostClient::new(server, enrolled.credential.token)?;
```

Keep the token: it is shown once. Every later call sends it as a bearer
token with the `aw-host-protocol` header.

## 2. Describe

Say what you are and what you run (`client.describe`): your agents (each by
a `hostAgentRef` of your choosing), what you offer to set up, and the
actions people can ask of you or of one agent (`needs`: `confirm` or
`panel`). Describe again whenever that changes, and when a heartbeat's
answer has `describeWanted`. A workspace manager then binds each of your
agents to an agent account (`host_agent_bind`); until then you run nothing
for it.

## 3. Read commands

Long-poll `client.commands(after, wait)`, fetch each command
(`client.command(id)`), carry it out and acknowledge it once
(`client.ack`). Use the answer's `next` as the next `after`.

| Command | What to do | Acknowledge with |
|---|---|---|
| `start` | Start the run for `agent.hostAgentRef` with `input.prompt` | `accepted` and your `hostRunRef`, or `refused` with a reason |
| `cancel` | Stop the run | `done` once stopped |
| `steer` | Pass the message (`messageId`) to the running agent | `accepted` |
| `answer` | Hand the agent the answer to its request (`answer`) | `accepted` |
| `inspect` | Say whether you know the run | `done` with `{"known": …, "hostRunRef"?, "state"?}` |
| `action` | Carry out the action (`action`, `hostAgentRef`?) | `done` with a `result`, or `refused` |

**A start may be offered again.** If your acknowledgement is lost, the same
start (the same `startKey`) comes back. Remember every start key you have
started and answer a repeat with the run you already started; never start it
twice. If you can't tell whether you started it, acknowledge `uncertain`.

## 4. Report on runs

A start carries `workspaceCredential`, a short-lived token for that run.
Report with it through the run operations on the public API
(`client.reporter(run_id, &credential)`): `run_activity_report`
(`run_started`, `text_message`, an `interrupt` that raises a request),
`run_usage_report`, `run_result_report` and `run_update` (the run's state:
`working`, `done`, `failed`, `cancelled`). Give each report its own key so
sending it again changes nothing.

Send a heartbeat (`client.heartbeat`) at your report interval (15 seconds
to an hour), listing every run you are still doing. A run you stop listing
for three heartbeats is marked interrupted.

## 5. Notices

Tell the workspace when one of your agents changes (`client.notice`):
`agent_profile_changed` (its name), `agent_ref_changed` (a new
`hostAgentRef`; the same `ref.lineage` means the same agent) and
`agent_retired` (its runs are interrupted).

## Check it

```text
cargo run -p agent-workspace-host-conformance -- host -- <command that starts your host>
```

serves a stand-in workspace on a local port, starts your host with
`AW_SERVER` and `AW_ENROL_CODE` pointing at it, and drives it through every
command, a lost acknowledgement and the notices. It prints what passed and
what is missing, and exits 1 if anything failed. `host-conformance server
--server <url> --code <code>` runs the same suite the other way, acting as
a host against a server.
