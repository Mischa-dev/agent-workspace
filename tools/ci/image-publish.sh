#!/bin/sh
# Pushes one architecture's image as <image>:<commit>-<arch>; the workflow's
# manifest job joins them under the release tag.
# Usage: tools/ci/image-publish.sh <local image:tag> <arch>
set -eu
local_image=${1:?image}
arch=${2:?arch}
image=$(printf '%s' "${IMAGE:?}" | tr '[:upper:]' '[:lower:]')
printf '%s' "${TOKEN:?}" | docker login ghcr.io -u "${GITHUB_ACTOR:?}" --password-stdin
docker tag "$local_image" "$image:$GITHUB_SHA-$arch"
docker push "$image:$GITHUB_SHA-$arch"
