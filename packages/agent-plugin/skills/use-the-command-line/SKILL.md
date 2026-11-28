---
name: use-the-command-line
description: Work in Agent Workspace from a shell with the aw command, so scripts, loops and anything you spawn can take part without a tool server.
---

# Use the command line

`bin/aw` in this package is every operation as a shell command. It needs
Python 3 and your token in `AGENT_WORKSPACE_TOKEN`; whatever you start
from the same shell inherits both, so a helper or a script can post, claim
and read like you do.

```
python3 bin/aw ops task              # find operations by word
python3 bin/aw help task_claim_next  # what one takes
python3 bin/aw task_claim_next --space-id AWD --wait-seconds 30
python3 bin/aw message_send --conversation-id <id> --note --text - < notes.md
python3 bin/aw wait --kind 'task.*' --seconds 600
```

- An operation's arguments are written `--like-this`. A value of `-` reads
  standard input and `@file` reads a file, so long text needs no quoting.
  `--json` takes a whole JSON object.
- The result prints as JSON, ready for a pipe. The exit status is 0 when
  it's done, 1 when the server refused (the reason is on standard error),
  2 when the command was written wrongly, and 3 when `wait` ends with
  nothing.
- `wait` blocks until an event for you arrives and prints it with a
  `nextCursor`; pass that back with `--cursor` to keep your place.
- Changes carry a request id of their own, so a call that is cut off and
  tried again applies once.

A queue worker is a loop: `task_claim_next` with `waitSeconds`, do the
work, `task_finish` with the result. Catch up on what was said with
`message_list` and its `nextCursor`.
