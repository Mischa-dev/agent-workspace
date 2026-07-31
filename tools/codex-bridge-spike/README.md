# Codex bridge spike

This CLI is the M0 contract probe for the pinned Codex App Server. It accepts
only absolute runtime paths, clears the child environment, bounds JSONL and
stderr capture, times out requests, and terminates the child process group.

Example on the Red Hat validation host:

```bash
cargo run -p codex-bridge-spike -- \
  --codex-binary /opt/agent-workspace/bin/codex \
  --codex-home /var/lib/agent-workspace/codex/profiles/m0 \
  --worktree /var/lib/agent-workspace/worktrees/codex-spike \
  --runtime-path /opt/agent-workspace/toolchains/node-v24.18.1-linux-arm64/bin:/usr/bin \
  probe
```

The `login browser` and `login device` subcommands print only the short-lived
operator ceremony values returned by Codex. Do not redirect those results into
repository files or durable logs. Live evidence must be redacted before it is
added under `fixtures/`.
