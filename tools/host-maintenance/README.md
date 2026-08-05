# Agent Workspace host maintenance

This owner-operated binary creates and validates credential-free ordinary
backups and restores them only into a new database plus empty drill roots. It
is part of the M8 private-host boundary and is not an in-place recovery tool.

The binary accepts absolute paths only. The database name is read from an
owner-only regular file and passed to PostgreSQL children only through their
cleared child environment; URLs and credentials are rejected and the value is
never placed in arguments or output. The checked-in Red Hat wrappers run the
pinned PostgreSQL 18 tools as the container postgres operating-system user,
connect as the `agent_workspace` database role, and preserve the existing
`bot` rootless Podman project even when the maintenance binary runs under
`sudo` to read the protected application data.

## Commands

    agent-workspace-host-maintenance backup \
      --destination /var/backups/agent-workspace/2026-08-04 \
      --data-root /var/lib/agent-workspace \
      --release-root /opt/agent-workspace/releases/<full-git-sha> \
      --release-commit <full-git-sha> \
      --database-name-file /etc/agent-workspace/maintenance-database \
      --pg-dump /opt/agent-workspace/releases/<full-git-sha>/bin/agent-workspace-pg-dump \
      --psql /opt/agent-workspace/releases/<full-git-sha>/bin/agent-workspace-psql \
      --tar /usr/bin/tar

    agent-workspace-host-maintenance validate \
      --source /var/backups/agent-workspace/2026-08-04

    agent-workspace-host-maintenance restore \
      --source /var/backups/agent-workspace/2026-08-04 \
      --target-data-root /var/lib/agent-workspace-restore-drill \
      --target-release-root /opt/agent-workspace-restore-drill \
      --target-database-name-file /etc/agent-workspace/restore-database \
      --active-database-name-file /etc/agent-workspace/maintenance-database \
      --pg-restore /opt/agent-workspace/releases/<full-git-sha>/bin/agent-workspace-pg-restore \
      --psql /opt/agent-workspace/releases/<full-git-sha>/bin/agent-workspace-psql \
      --tar /usr/bin/tar \
      --chown /usr/bin/chown

    agent-workspace-host-maintenance storage-report \
      --data-root /var/lib/agent-workspace \
      --releases-root /opt/agent-workspace/releases \
      --backups-root /var/backups/agent-workspace \
      --database-name-file /etc/agent-workspace/maintenance-database \
      --psql /opt/agent-workspace/releases/<full-git-sha>/bin/agent-workspace-psql \
      --data-quota-bytes 21474836480 \
      --minimum-free-bytes 5368709120

    # Dry run is the default. Mutation requires the literal opt-in below.
    agent-workspace-host-maintenance retention-cleanup \
      --database-name-file /etc/agent-workspace/maintenance-database \
      --psql /opt/agent-workspace/releases/<full-git-sha>/bin/agent-workspace-psql

    agent-workspace-host-maintenance retention-cleanup \
      --database-name-file /etc/agent-workspace/maintenance-database \
      --psql /opt/agent-workspace/releases/<full-git-sha>/bin/agent-workspace-psql \
      --apply true

The destination parent and backup source must deny group and other access.
Run the command as root through the existing passwordless `bot` operator path;
the database-name files contain only `agent_workspace` or the explicitly
created drill database name and use mode `0600`.
The supported source data root is exactly `/var/lib/agent-workspace`. Database
path identities remain canonical to that logical root. A clean drill therefore
starts its separate service in a private mount namespace that bind-mounts the
restored test root over `/var/lib/agent-workspace` for that service only; it
must never rewrite path identity rows or expose the active data tree.
Symlinks, existing destinations, source/destination containment, unknown
archive paths, checksum mismatches, PostgreSQL versions other than 18, an
incomplete credential-table exclusion set, a non-empty target database, and a
target database matching the active connection are rejected.

The storage report returns only aggregate allocated bytes and entry counts; it
does not print paths below the registered roots. Registered roots and their
path components may not be symlinks. Internal symlinks are counted and skipped,
never followed. The report separately identifies the
PostgreSQL database, unique application data, rebuildable repository mirrors,
provider profiles, temporary files, immutable releases, and backup archives.
Each category labels unique preserved data, rebuildable cache, sensitive or
reclaimable operational data, rollback artifacts, and recovery artifacts.
The report rejects symlinks and reports whether both the data quota and
minimum-free-space watermark admit new growth.

Retention cleanup uses policy version 1. It reports eligible row counts unless
`--apply true` is present. Apply runs under a transaction-level advisory lock
and emits the same category-only summary. It removes only expired operational
reservations and sufficiently old OAuth token material, published outbox rows,
and completed GitHub jobs. An active expired invitation is revoked in place so
its issuer, Actor attribution, and membership history remain intact. Durable
audit events, Messages, Work Items, approvals, Agent Runs, and provider-session
identity are never deleted by this command.

Ordinary backups include the immutable application release and the registered
artifact, attachment, repository, and worktree roots. The manifest classifies
repository mirrors as rebuildable caches and artifacts, attachments, and
worktrees as preserved data; dirty and unpushed worktrees are never pruned by
this command. Ordinary backups exclude browser
sessions, password credentials, MCP bearer credentials, OAuth grants and token
rows, OAuth client registrations, Codex profile contents, and external
file-backed secrets. Because browser session rows are absent, retained audit
events keep their Actor and action history but clear the optional session
correlation before post-data foreign keys are installed. A restored
instance starts with all human passwords absent, sessions absent, MCP/OAuth
grants absent, authentication epochs advanced, active invitation capabilities
revoked, and Execution Profiles logged out. Restored data is assigned to the
locked `agent-workspace` service account while the restored release stays
root-owned and readable. Host-local credential reset and provider
reauthentication are mandatory.
