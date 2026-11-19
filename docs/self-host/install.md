# Install

## What you need

- A Linux machine (amd64 or arm64) with Docker and Compose, or Podman and
  `podman-compose`. 2 GB of memory and 10 GB of disk is enough to start;
  see [small machines](small-machines.md) for less.
- This repository, or just its `deploy/container/` folder and a published
  image.

## Start it

```sh
cd deploy/container
./init.sh
```

`init.sh` writes `.env` and the database's secret files with random values
(never replacing ones that exist) and starts the server and PostgreSQL.
The server migrates the database, then serves on `http://localhost:3100`.
`docker compose ps` shows the app healthy once it is ready.

To use a published image rather than building one, set `AW_IMAGE` and
`AW_VERSION` in `.env` (for example the release you want) before
`./init.sh`.

## First run in the browser

While nobody has an account, the server keeps a one-time setup code in its
data folder and logs where it is. Read it with:

```sh
docker compose exec app cat /var/lib/agent-workspace/setup-code
```

Open the app, enter the code, and make the first account. Then either make
a workspace (choosing a shape for it: a small team, a personal space and
the like) or upload an archive exported from another server
([moving a workspace](moving-a-workspace.md)). The code works once; the
file goes when it has been used. Only someone who can read the server's
data folder can read it.

If the server was restarted with more than one process and the code is on
one that has gone, `docker compose exec app agent-workspace-server
setup-code` makes a new one where you can read it.

On a machine without a browser to hand, the first account can still be
made from a terminal:

```sh
docker compose exec app agent-workspace-server bootstrap-admin \
  --login-name <login> --display-name "<your name>" \
  --organization-name "<workspace name>" --organization-slug <address>
```

## Next

- Serve it beyond localhost: [TLS and a public address](tls.md).
- Turn on daily backups: `docker compose --profile backup up -d`
  ([backups](backups.md)).
- Optional services (mail for invitations and resets, single sign-on,
  GitHub, an object store for files) are each a few settings in
  [configuration](configuration.md).
