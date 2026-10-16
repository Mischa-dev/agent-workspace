# Backups

The daily backup makes an encrypted copy of everything that cannot be rebuilt,
copies it off the host, and prunes old copies. A restore drill proves that the
off-host copy can be restored.

## What is backed up

Each run writes one **set**: a directory named by its UTC start time, such as
`20261016T033000Z`.

| File | Contents |
| --- | --- |
| `database.dump.age` | `pg_dump --format=custom` of the live database, encrypted. |
| `data.tar.gz.age` | Data root paths that cannot be rebuilt, encrypted. By default: `attachments`, `artifacts` and `codex/profiles` (provider account homes). |
| `backup-info` | Plain text: set time, database name, latest migration version, release. No secrets. |
| `SHA256SUMS` | Checksums of the three files above. |

Left out on purpose: `repositories` and `worktrees` (re-cloned from GitHub),
`logs` and `temporary`.

The dump is streamed straight into `age`, so no plaintext copy is written to
disk. Sets are encrypted to an age **public** key. The host never needs the
private key to make a backup.

The **secrets master key** (`AGENT_WORKSPACE_SECRETS_KEY_PATH`) is backed up
on its own, by `agent-workspace-backup-key`:

- It is encrypted to a **different** age recipient.
- It goes to a **different** off-host place.
- It is uploaded only when the file changed (the script keeps its hash in
  `/var/lib/agent-workspace-backup`).
- Key sets are never pruned. An old dump may need the key that was current
  when it was made.

The scripts refuse to run if the two recipients match, or if one remote path
sits inside the other on the same host. A dump next to its key defeats the
encryption.

## Where it goes

- Local copies: `/var/backups/agent-workspace/encrypted/<set>/`.
- Off-host: `BACKUP_REMOTE` (database sets) and `BACKUP_KEY_REMOTE` (key).

A remote is either `ssh:[user@]host:/path` (rsync over ssh, the default) or
`local:/path` (a directory on this machine; for tests or a removable disk).
Uploads land as `<set>.partial` and are renamed when complete. If an upload
fails, the next run uploads the missed set too. A local set is pruned only
when the remote also holds it.

## Retention

Sets older than `BACKUP_LOCAL_KEEP_DAYS` (default 7) are deleted locally, and
sets older than `BACKUP_REMOTE_KEEP_DAYS` (default 30) are deleted off-host.
The newest `BACKUP_KEEP_MIN` sets (default 3) are always kept on both sides.

## Files

All in `deploy/redhat/`:

- `agent-workspace-backup`: the daily backup.
- `agent-workspace-backup-key`: the secrets key backup.
- `agent-workspace-restore-drill`: the restore drill.
- `agent-workspace-backup-lib.sh`: shared helpers, including the transport.
  All off-host copying goes through its `remote_*` functions.
- `backup.env.example`: every setting, with comments.
- `agent-workspace-daily-backup.service` and `.timer`: run both backup
  scripts at 03:30 with low CPU and I/O priority and a 1 GiB memory cap.
  `pg_dump` itself runs inside the PostgreSQL container, outside those limits.

The scripts read `/etc/agent-workspace/backup.env` (or `--config FILE`). The
file is parsed as plain `KEY=value` lines; it is never sourced. A variable set
in the environment overrides the file.

## Host setup

Do these once on the Red Hat host.

1. **Install age.** It is in EPEL:

   ```bash
   sudo dnf install -y https://dl.fedoraproject.org/pub/epel/epel-release-latest-9.noarch.rpm
   sudo dnf install -y age rsync
   age --version
   ```

   On RHEL proper, EPEL may also need
   `sudo subscription-manager repos --enable codeready-builder-for-rhel-9-$(arch)-rpms`.
   If EPEL is not an option, install the release binary from
   `https://github.com/FiloSottile/age/releases` after checking its checksum.

2. **Make two key pairs, off the host** (on the Mac: `brew install age`):

   ```bash
   age-keygen -o agent-workspace-backup-data.key
   age-keygen -o agent-workspace-backup-secrets-key.key
   ```

   Each command prints its public key (`age1...`). Keep both private files in
   the password manager or offline, in different places. Do not copy either
   private key to the host for daily use.

3. **Write the public keys on the host:**

   ```bash
   echo 'age1...data...' | sudo install -m 0600 /dev/stdin /etc/agent-workspace/backup-recipients.txt
   echo 'age1...key...'  | sudo install -m 0600 /dev/stdin /etc/agent-workspace/backup-key-recipients.txt
   ```

4. **Set up the off-host targets.** Make a backup user on each target machine
   that owns only its backup directory. Use different machines, or at least
   different users and directories, for the data and the key. Then on the
   host:

   ```bash
   sudo install -d -m 0700 /etc/agent-workspace/backup-ssh
   sudo ssh-keygen -t ed25519 -N '' -f /etc/agent-workspace/backup-ssh/id_ed25519
   sudo ssh-keyscan -p 22 TARGET_HOST | sudo tee /etc/agent-workspace/backup-ssh/known_hosts
   ```

   Check the scanned fingerprint against the target, then add
   `id_ed25519.pub` to the backup user's `authorized_keys` on each target. The
   scripts run only `mkdir`, `ls`, `mv` and `rm` there, plus rsync.

5. **Write the config:**

   ```bash
   sudo install -m 0600 deploy/redhat/backup.env.example /etc/agent-workspace/backup.env
   sudoedit /etc/agent-workspace/backup.env
   ```

   Set `BACKUP_REMOTE`, `BACKUP_KEY_REMOTE`, and `BACKUP_SECRETS_KEY_FILE` (the
   same path as `AGENT_WORKSPACE_SECRETS_KEY_PATH`).

6. **Install the scripts and units.** `deploy-release.sh` copies the four
   backup scripts into the release `bin` directory with the other wrappers,
   so deploy a release that has them first. Then:

   ```bash
   sudo install -m 0644 deploy/redhat/agent-workspace-daily-backup.service \
     deploy/redhat/agent-workspace-daily-backup.timer /etc/systemd/system/
   sudo systemctl daemon-reload
   sudo systemctl enable --now agent-workspace-daily-backup.timer
   ```

7. **Run it once and check:**

   ```bash
   sudo systemctl start agent-workspace-daily-backup.service
   journalctl -u agent-workspace-daily-backup.service --since '-15 minutes' --no-pager
   ```

8. **Run the restore drill** (below). When it passes, delete the old
   unencrypted `daily-*.dump` files in `/var/backups/agent-workspace` by hand.

## Restore drill

The drill fetches the newest set from `BACKUP_REMOTE`, checks `SHA256SUMS`,
creates the spare database `BACKUP_DRILL_DATABASE` (default
`agent_workspace_restore_drill`), restores the dump into it, reads the whole
data archive, and prints a report: counts of organizations, projects and
messages, and the latest migration version. It fails if the migration version
differs from the one recorded at backup time. It then drops the spare
database.

It refuses to restore into the live database, `postgres`, or a template. If
the spare database already exists it stops, unless you pass `--replace`.

The drill needs the private **data** key. Bring it to the host only for the
drill and remove it after:

```bash
sudo install -m 0600 /dev/stdin /root/drill-identity.txt   # paste, then Ctrl-D
sudo /opt/agent-workspace/current/bin/agent-workspace-restore-drill \
  --identity /root/drill-identity.txt
sudo shred -u /root/drill-identity.txt
```

Options:

- `--keep` keeps the spare database for a look around.
- `--set 20261016T033000Z` picks a set instead of the newest.
- `--from local` uses the local copies instead of the off-host ones.
- `--database NAME` picks a different spare database name.

Run the drill after the first setup, after changing the backup settings, and
every few months.

## A real restore

The drill only checks a backup. For a real recovery, do the same steps by
hand into a **new** database and an empty data location, then switch over:

```bash
age -d -i data.key database.dump.age \
  | PGDATABASE=NEW_DB agent-workspace-pg-restore --dbname=NEW_DB --no-owner --no-privileges --exit-on-error
age -d -i data.key data.tar.gz.age | tar -xzf - -C /path/to/empty/data-root
(umask 077; age -d -i secrets-key.key -o keyring secrets-key.age)   # then install at AGENT_WORKSPACE_SECRETS_KEY_PATH
```

Restored files must be owned by `agent-workspace`. Provider profiles may still
need a fresh login.
