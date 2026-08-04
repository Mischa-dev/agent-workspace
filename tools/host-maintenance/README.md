# Agent Workspace host maintenance

This owner-operated binary creates and validates credential-free ordinary
backups and restores them only into a new database plus empty drill roots. It
is part of the M8 private-host boundary and is not an in-place recovery tool.

The binary accepts absolute paths only. The libpq database target is read from
an owner-only regular file and passed to PostgreSQL children only through their
cleared child environment; it is never placed in arguments or output. The
checked-in Red Hat wrappers run the pinned PostgreSQL 18 tools as the container
postgres user through the retained rootless Podman project, so the target file
may contain only the database name used through the local container socket.

## Commands

    agent-workspace-host-maintenance backup \
      --destination /var/backups/agent-workspace/2026-08-04 \
      --data-root /var/lib/agent-workspace \
      --release-root /opt/agent-workspace/releases/<full-git-sha> \
      --release-commit <full-git-sha> \
      --database-url-file /etc/agent-workspace/database-url \
      --pg-dump /opt/agent-workspace/current/bin/agent-workspace-pg-dump \
      --psql /opt/agent-workspace/current/bin/agent-workspace-psql \
      --tar /usr/bin/tar

    agent-workspace-host-maintenance validate \
      --source /var/backups/agent-workspace/2026-08-04

    agent-workspace-host-maintenance restore \
      --source /var/backups/agent-workspace/2026-08-04 \
      --target-data-root /var/lib/agent-workspace-restore-drill \
      --target-release-root /opt/agent-workspace-restore-drill \
      --target-database-url-file /etc/agent-workspace/restore-database-url \
      --active-database-url-file /etc/agent-workspace/database-url \
      --pg-restore /opt/agent-workspace/current/bin/agent-workspace-pg-restore \
      --psql /opt/agent-workspace/current/bin/agent-workspace-psql \
      --tar /usr/bin/tar

The destination parent and backup source must deny group and other access.
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
out. Host-local credential reset and provider reauthentication are mandatory.
