#!/bin/sh
# Starts the server image with deploy/container/compose.yml the way a new
# operator would (init.sh), and waits for /health/ready (H1.13).
# Usage: tools/ci/image-smoke.sh <local image:tag>
set -eu
image=${1:?image}
cd "$(dirname "$0")/../../deploy/container"
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
docker compose down -v
rm -rf secrets .env
exit "$status"
