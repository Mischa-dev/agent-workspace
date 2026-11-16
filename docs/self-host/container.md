# Running the container image

`deploy/container/` holds everything to run Agent Workspace on one machine
with Docker Compose or Podman Compose: the image (`Containerfile`), the
Compose file, a first-start script and the database's first-run setup.

## First start

```sh
cd deploy/container
./init.sh
```

`init.sh` writes `.env` and three secret files under `secrets/` with random
values (it never replaces ones that exist), then runs `docker compose up -d`
(or `podman-compose up -d`). When `docker compose ps` shows the app healthy,
open the address in `.env` (`http://localhost:3100` by default) and create the
first account with `docker compose exec app agent-workspace-server
bootstrap-admin ...` (see the README).

To serve anything but localhost, put a TLS proxy in front and set
`AW_PUBLIC_ORIGIN` to its `https://` address in `.env`. Every other setting is
an `AGENT_WORKSPACE_*` variable (`docs/self-host/configuration.md`); add them to
the `x-app-environment` block of `compose.yml`.

## What runs

- `db`: PostgreSQL 18. On its first start, `initdb/` makes the role the
  server uses, `aw_app`, which owns the schema but is not a superuser, so
  row security applies to it.
- `app`: the server. With no arguments it migrates and serves. While it
  migrates, `/health/ready` answers 503 with the step it's on; it also answers
  503 if the database holds a migration newer than the image (an older image
  started against a newer database). The container's health check is
  `agent-workspace-server health`.
- `backup` (profile `backup`): a backup set every day (`docs/self-host/backups.md`).

Until the host program is its own image, agent runs use the built-in host
inside the server.

## Building the image

```sh
docker build -f deploy/container/Containerfile -t agent-workspace .
```

from the repository root. The image carries the server, the built web app,
`/app/NOTICE` and `/app/THIRD-PARTY-NOTICES`. The `Image` workflow builds it
for amd64 and arm64 on a release tag (and on demand), starts it with this
Compose file to check it gets ready, and on a tag publishes it.
