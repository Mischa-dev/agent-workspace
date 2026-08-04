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

The destination parent and backup source must deny group and other access.
Run the command as root through the existing passwordless `bot` operator path;
the database-name files contain only `agent_workspace` or the explicitly
created drill database name and use mode `0600`.
Symlinks, existing destinations, source/destination containment, unknown
archive paths, checksum mismatches, PostgreSQL versions other than 18, an
incomplete credential-table exclusion set, a non-empty target database, and a
target database matching the active connection are rejected.

Ordinary backups include the immutable application release and the registered
artifact, attachment, repository, and worktree roots. They exclude browser
sessions, password credentials, MCP bearer credentials, OAuth grants and token
rows, Codex profile contents, and external file-backed secrets. A restored
instance starts with all human passwords absent, sessions absent, MCP/OAuth
grants absent, authentication epochs advanced, and Execution Profiles logged
out. Restored data is assigned to the locked `agent-workspace` service account
while the restored release stays root-owned and readable. Host-local credential
reset and provider reauthentication are mandatory.
