#!/bin/sh
# Starts the server image with deploy/container/compose.yml the way a new
# operator would (init.sh), and waits for /health/ready (H1.13).
# Usage: tools/ci/image-smoke.sh <local image:tag>
set -eu
image=${1:?image}
here=$(cd "$(dirname "$0")" && pwd)
cd "$here/../../deploy/container"
export AW_IMAGE="${image%:*}" AW_VERSION="${image##*:}"
./init.sh
status=1
for _ in $(seq 1 60); do
  if curl -fsS http://127.0.0.1:3100/health/ready; then
    status=0
    break
  fi
  sleep 5
done
docker compose exec -T app agent-workspace-server health || status=1
docker compose logs --no-color app | tail -n 50

# H1.19: the same stack again on a network with no way out. A self-hosted
# server needs no outside service to start and get ready.
if [ "$status" = 0 ]; then
  docker compose down
  offline="$here/offline.compose.yml"
  docker compose -f compose.yml -f "$offline" up -d
  status=1
  for _ in $(seq 1 60); do
    if docker compose -f compose.yml -f "$offline" exec -T app agent-workspace-server health; then
      status=0
      break
    fi
    sleep 5
  done
  docker compose -f compose.yml -f "$offline" logs --no-color app | tail -n 20
  docker compose -f compose.yml -f "$offline" down
fi
docker compose down -v
rm -rf secrets .env
exit "$status"
