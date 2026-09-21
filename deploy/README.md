# Portable private deployment

The Containerfile and Podman runner prepare a Linux deployment without copying any Codex login, host home directory, repository checkout, or existing application secrets. The active Red Hat deployment and its live verification are documented in [REDHAT.md](REDHAT.md).

Requirements: rootless Podman, Node 22 or 24 for the operator script, enough disk for PostgreSQL and uploaded files, and an approved host. The application image provides its own Node 24 runtime. The build uses Docker image format because OCI format discards HEALTHCHECK metadata. Run from this repository:

```sh
node scripts/deploy-podman.mjs prepare
node scripts/deploy-podman.mjs build
node scripts/deploy-podman.mjs start
node scripts/deploy-podman.mjs status
```

`prepare` creates a new private `.local/deployment/runtime.env` and `owner-token.txt` plus a separate work-hook credential; it refuses to overwrite existing configuration. It prints neither credential. Keep these files out of version control and backups shared with other people. `start` binds the application to **127.0.0.1:4310 only** and performs migrations before starting the application. PostgreSQL has no published host port. Named volumes preserve database and files when containers stop. The runner refuses to replace existing application containers.

Each long-running container has a default memory limit of 384 MiB, one CPU and 256 processes, with `unless-stopped` crash recovery. The server and worker use a 256 MiB JavaScript heap. Override limits with `WORKSPACE_DATABASE_MEMORY`, `WORKSPACE_SERVER_MEMORY`, `WORKSPACE_WORKER_MEMORY` and the corresponding `_CPUS` variables before initial creation. PostgreSQL uses 64 MiB shared buffers and at most 50 connections. Startup, resume and backup completion explicitly verify API and worker health. Periodic Podman health timers are disabled by default because some rootless hosts cannot register them; set `WORKSPACE_HEALTH_INTERVAL=30s` on a host with a working Podman/systemd timer connection to enable them. Temporary migration/archive containers disable the image healthcheck entirely. The worker has no HTTP listener; its health is reported by the server's database heartbeat check, so it does not inherit the image's HTTP health probe. `WORKSPACE_HOST_PORT` selects another loopback port for an isolated deployment.

For a remote domain, set `PUBLIC_ORIGIN=https://workspace.example` before preparing configuration. Add a trusted HTTPS reverse proxy after approving the host, preserve the public Host header, and proxy streaming responses without buffering. `BIND_HOST=0.0.0.0` is only inside the container. If forwarded client information is needed, add `TRUST_PROXY` with explicit proxy IPs/CIDRs to the private env file; never trust arbitrary forwarded headers. Secure owner cookies follow `PUBLIC_ORIGIN`. Browser-based MCP clients on another origin require an explicit comma-separated `AGENT_ALLOWED_ORIGINS`; server-to-server clients normally send no Origin header. Do not expose the container port directly on the public interface.

The image includes the web/API service and scheduler. It does **not** include a qualified Linux Codex runtime or login. External agents can use polling, webhook wakeups, HTTP, or MCP. Managed Codex chat and repository execution remain unavailable until that host/runtime is qualified separately. Do not mount a personal home directory or copy a desktop authentication store to make the indicator green.

## Back up

Run `node scripts/deploy-podman.mjs backup` for the executable backup path. It checks which application containers are running, briefly stops only those, writes the database and full file volume into a new private backup directory, checks both archives, creates a SHA-256 manifest, and restarts the same containers in a `finally` block, then waits for API/worker health. It preserves partial output on error and never deletes an old backup. The manifest is written only after successful verification. Config, owner-token and work-hook credential files are copied into the private backup when present. All artifacts use mode 600 inside mode 700 directories. The database contains private conversations, credential hashes and webhook signing secrets; protect the entire backup.

Equivalent manual steps for diagnosis:

```sh
umask 077
mkdir -p .local/backups/portable
podman stop workspace-worker workspace-server
podman exec workspace-postgres pg_dump -U workspace -d workspace -Fc > .local/backups/portable/database.dump
podman run --rm --volume workspace-files:/var/lib/workspace:ro localhost/agent-workspace:connected tar -C /var/lib/workspace -czf - . > .local/backups/portable/files.tar.gz
podman start workspace-server workspace-worker
podman exec -i workspace-postgres pg_restore --list < .local/backups/portable/database.dump
tar -tzf .local/backups/portable/files.tar.gz > /dev/null
```

Verify both command exit statuses before considering the backup complete; if either backup fails, preserve the partial outputs and restart the stopped application containers deliberately. Generate SHA-256 checksums and keep them alongside the archives. File archives include the complete application data volume, not only currently referenced uploads.

## Reboot and daily backups

`deploy/systemd/` contains generic templates. Rendered versions are installed and active on Red Hat; use the following steps for another host. After the host is approved and the initial deployment works, replace `/home/OWNER/agent-workspace`, the deployment directory and `/usr/bin/node` with the actual repository, private configuration and Node executable paths. Copy the three files to `~/.config/systemd/user/`, then review them before running:

```sh
systemctl --user daemon-reload
systemctl --user enable --now workspace.service
systemctl --user enable --now workspace-backup.timer
systemctl --user list-timers workspace-backup.timer
```

The workspace service calls the runner's `resume` action, which starts the existing database, waits for readiness, then starts the existing server and worker. It preserves their configured volumes and loopback port binding. The timer runs daily near 03:00 local time, with up to fifteen minutes of jitter and catch-up after a missed run. Check `journalctl --user -u workspace-backup.service`; a failed backup exits nonzero. Retention is manual; no automatic deletion is configured.

For rootless services to run after reboot without an interactive login, the host administrator must enable lingering for this **specific deployment user**, for example `loginctl enable-linger USERNAME`. Review that persistent-host change before applying it. On Red Hat, service startup and the actual backup/restore path are verified. A full shared-host reboot was deliberately not performed. Qualify each additional host separately.

## Restore into an isolated deployment first

Do not run restore commands against the existing database or volumes. Use a distinct container prefix (for example `workspace-restore`), network, database volume, file volume, configuration directory, and unpublished port. Start **only PostgreSQL** first, then:

1. Restore the database archive with `pg_restore --exit-on-error --no-owner --no-acl -U workspace -d workspace` through `podman exec -i workspace-restore-postgres`. The target database must be empty.
2. Restore `files.tar.gz` into the new file volume, using the image's `node` UID and the volume `:U` mapping. Preserve permissions.
3. Before starting the server or worker, set `settings.dispatchEnabled` to JSON `false`, delete restored `owner_sessions`, revoke **all** restored `agent_credentials` by setting `revoked_at=now()`, delete `oauth_state` (clients, grants and refresh credentials), and mark prepared/dispatching/running runs and claimed requests for reconciliation. Cancel pending response requests so restore never dispatches them automatically. Check whether the credential/OAuth tables exist first when restoring an archive from an older release. Preserve run history and do not assume a missing receipt means no external work happened.
4. Start only the server on an unused loopback port. Sign in using the new deployment's owner token. Check record counts, recent Resources, and several downloads/checksums; inspect uncertain runs and credentials. Revoke stale agent credentials or rotate webhook secrets when appropriate.
5. Start the worker while dispatch remains disabled. Confirm its heartbeat and inspect its recovery state. Resume dispatch explicitly only after review.

The existing local `pnpm backup`/`pnpm restore` workflow is for the local runner; do not point its macOS PostgreSQL commands at this container deployment. The Red Hat release completed an isolated container restore as recorded in `REDHAT.md`; verify the same steps on any other target. Promotion to a public hostname and removal of old volumes remain separate operator actions.
