# Backups and restore

The server binary makes and restores its own backups, so any host that can
run it can back it up: a container, a small VM or a laptop. It needs
`pg_dump` and `pg_restore` of the same or a newer major version than the
database, `tar`, and `age` if the sets are encrypted.

The Red Hat host's scripts in `deploy/redhat/` write the same set format and
add the off-host copy; see `docs/operations/backups.md`.

## What a set holds

`backup create` writes one **set**: a folder named by its UTC start time,
such as `20261114T033000Z`, inside the folder given with `--to`.

| File | Contents |
| --- | --- |
| `database.dump` | `pg_dump --format=custom` of the database. |
| `data.tar.gz` | The data root folders that can't be rebuilt: by default `attachments`, `artifacts`, `files` and `codex/profiles`. |
| `backup-info` | Plain text: set time, migration version, release, the folders kept. No secrets. |
| `SHA256SUMS` | Checksums of the files above. |

With `--recipient-file`, the dump and the archive are streamed through
`age` and end in `.age`; nothing unencrypted is written. The set is built
in `<name>.partial` and renamed when complete, so a set folder without that
suffix is always whole.

The database password reaches `pg_dump` and `pg_restore` in their
environment, never as an argument. The dump holds the key rotation lock, so
it sees every secret either before or after a rotation, never half of each.

Each run writes `backup-status.json` in the data root; the Server page and
`preflight` read it.

## Commands

```sh
# One set now.
agent-workspace-server backup create --to /backups [--recipient-file recipients.txt] \
  [--data-paths attachments,artifacts,files,codex/profiles]

# A set every day at 03:30 UTC, keeping 14 days (and always the newest 3).
agent-workspace-server backup schedule --to /backups --at 03:30 --keep-days 14 \
  [--recipient-file recipients.txt]

# The secrets keyring, encrypted to a different recipient.
agent-workspace-server backup key --to /backups-key --recipient-file key-recipients.txt \
  --database-recipient-file recipients.txt
```

`backup` reads the same configuration as the server (the database URL,
`AGENT_WORKSPACE_DATA_ROOT`, the keyring path). It runs before migrations,
so it backs up the database as it is.

The keyring opens every stored secret. Keep its backup apart from the
database sets: `backup key` refuses a recipient that also opens them. Key
sets are never pruned; an old dump may need the key that was current when it
was made.

## Restore

```sh
agent-workspace-server restore --from /backups/20261114T033000Z \
  --database-url-file target-url.txt --data-root /srv/agent-workspace/data \
  [--identity-file identity.txt]
```

`restore` checks the set's checksums, then restores the database into the
target and the folders into the data root. It refuses a target database that
holds any table, and a data root that holds anything: restore into empty
ones. The target URL is read from a file, so its password doesn't show in a
process list. It needs no other configuration.

Then point the server at the restored database and data root and start it:
migrations bring an older set up to date. Restore the keyring from its own
set (`age --decrypt`) to the path in `AGENT_WORKSPACE_SECRETS_KEY_PATH`,
owner-only.

## Restore drill

```sh
agent-workspace-server restore drill --from /backups/20261114T033000Z \
  --database-url-file spare-url.txt [--identity-file identity.txt] [--keyring /path/to/keyring]
```

The drill restores into a spare, empty database, reads the data archive end
to end, and checks the counts, the migration version, and that the keyring
opens every restored secret. Drop the spare database afterwards. Run it
after any change to how backups are made, and now and then besides.

## With Docker Compose

Run the same image with the `backup` command beside the server, sharing its
configuration and data volume and with a volume for the sets, for example
`command: ["backup", "schedule", "--to", "/backups", "--at", "03:30"]`. The
packaged Compose file and its `backup` profile come with the container image
(H1.13).
