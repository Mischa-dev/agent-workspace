# Small machines

## What it needs

One process serves everything: the app, its API, live updates and the
background work. Beside it, PostgreSQL. Idle, the pair does a few dozen
short database transactions a second (its own polling), the same with one
workspace or ten thousand ([hosting shapes](../hosting/shapes.md)).

| | Starts on | Comfortable for a team |
| --- | --- | --- |
| Memory | 1 GB (server and database) | 2 to 4 GB |
| CPU | 1 core | 2 cores |
| Disk | 2 GB plus files | 10 GB plus files and backups |

Agent runs on the built-in host take what the agents themselves take;
count them separately.

## Running on less

- **Profile.** `AW_PROFILE=small` in `.env` (`AGENT_WORKSPACE_PROFILE`)
  sizes the database pool and the background work for a small machine
  ([configuration](configuration.md), "Profiles").
- **Files elsewhere.** Uploads can go to any S3-compatible object store
  (`AGENT_WORKSPACE_FILE_STORE=object`), so the machine's disk holds only
  the database, and downloads can go straight from the store
  (`files.redirect_downloads`).
- **Storage limits.** A workspace's own limit on stored files
  (`files.workspace_quota_mb`, in its settings) and the server's watermark
  (`AGENT_WORKSPACE_DATA_QUOTA_BYTES`, `AGENT_WORKSPACE_MINIMUM_FREE_BYTES`)
  refuse uploads before the disk fills; nothing already stored is touched.
- **Backups off the machine.** Point the backup volume at other storage,
  and keep the keyring's backup apart ([backups](backups.md)).
- **No timer at all.** With `AGENT_WORKSPACE_WORK_LOOP=off`, background
  work runs only when something calls the tick endpoint, so the process can
  sleep between requests (see the edge shape in
  [hosting shapes](../hosting/shapes.md)).
