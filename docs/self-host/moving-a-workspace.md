# Moving a workspace

A workspace can leave one server as one archive and arrive on another,
whole: its spaces, conversations, tasks, pages, records, files with their
bytes, people, agents and apps, and its history.

## Out

An owner exports it from Settings › Export (the "Export the workspace"
permission, owners only by default), or an agent or app with that
permission calls `workspace_export_start`. A job writes one zip archive to
the workspace's file store; it is kept seven days and only the person who
asked can download it. What never goes: direct messages and group
conversations (they belong to the people in them, not the workspace), and
the values of secrets and connections (the archive names what needs
setting up again, never the keys).

## In, on a new server

On a server nobody has signed in to yet: make the first account in the
browser ([install](install.md)), then choose to upload an archive instead
of making a workspace. The upload is yours alone: nobody else can send one
in your place, even with the setup code.

## In, on a server already running

```sh
agent-workspace-server workspace-import --file /path/to/archive.zip \
  --owner-login <login> [--slug <new address>]
```

(in the container: `docker compose exec app agent-workspace-server
workspace-import ...`, with the archive somewhere the container can read,
such as the data volume).

An import is checked whole before anything is written, then goes in
batches, saving where it got to. If it stops (a restart, a full disk), run
it again with `--resume <transfer id>` (the first run prints it) and it
goes on from there, ending in the same workspace as one uninterrupted run.

## What changes on the way

- **On a server that already has the archive's ids** (the same server, or
  a copy), the import is a copy: every id is new, and every reference in
  columns and in text points into the copy.
- **People** are matched to accounts here by verified email. Anyone not
  matched waits to be claimed: they get an account that is theirs once they
  sign in for it.
- **Agents and apps** arrive disabled ("needs connecting"); a manager
  connects them again.
- **Nothing an import writes sends anything**: no notification, email,
  webhook or event goes out for the history it brings in.
- A value that must be unique across the server and is already taken here
  (a login name, a workspace address) is skipped and counted in the
  import's report rather than renamed.

Usage counters and plan state stay with the server that counted them; the
new server counts from the import on.
