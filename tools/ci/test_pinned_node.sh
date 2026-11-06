#!/usr/bin/env bash
# Tests for how checks.sh gets the Node pinned in .node-version: the "Node
# toolchain" step (fetch_pinned_node) and the PATH set after it
# (activate_pinned_node). The functions are taken from checks.sh as they
# are and run against stub uname, curl, mv, corepack and host node commands
# and a local archive, so nothing is downloaded and no real cache is used.
#
#   tools/ci/test_pinned_node.sh
set -uo pipefail

here="$(cd "$(dirname "$0")" && pwd)"
tmp="$(mktemp -d)"
trap 'rm -rf "${tmp}"' EXIT
failures=0
pass() { echo "ok   $1"; }
fail() { echo "FAIL $1${2:+: $2}"; failures=$((failures + 1)); }

defs="$(sed -n '/^pinned_node_dir() {/,/^}/p; /^is_pinned_node() {/,/^}/p; /^fetch_pinned_node() {/,/^}/p; /^activate_pinned_node() {/,/^}/p' "${here}/checks.sh")"
[ "$(grep -c '() {' <<< "${defs}")" -eq 4 ] ||
  { echo "FAIL couldn't find the four Node functions in checks.sh"; exit 1; }

pin=99.1.2
name="node-v${pin}-linux-x64"
real_mv="$(command -v mv)"

# The official archive and its SHA-256 list, as nodejs.org would serve them.
fixtures="${tmp}/fixtures"
mkdir -p "${fixtures}/${name}/bin"
printf '#!/bin/sh\necho v%s\n' "${pin}" > "${fixtures}/${name}/bin/node"
chmod +x "${fixtures}/${name}/bin/node"
(cd "${fixtures}" && tar -cJf "${name}.tar.xz" "${name}" && rm -rf "${name}" &&
  sha256sum "${name}.tar.xz" > SHASUMS256.txt)

# Stubs: uname says x86_64; curl serves the fixtures and logs each call; mv
# fails when FAIL_MV is set; corepack does nothing; the host's node reports
# HOST_NODE.
stubs="${tmp}/stubs"
mkdir -p "${stubs}"
printf '#!/bin/sh\necho x86_64\n' > "${stubs}/uname"
cat > "${stubs}/curl" << EOF
#!/bin/sh
out=""; url=""
while [ \$# -gt 0 ]; do
  case "\$1" in -o) out="\$2"; shift 2 ;; -*) shift ;; *) url="\$1"; shift ;; esac
done
echo "\${url}" >> "${tmp}/curl.log"
cp "${fixtures}/\${url##*/}" "\${out}"
EOF
cat > "${stubs}/mv" << EOF
#!/bin/sh
[ -n "\${FAIL_MV:-}" ] && exit 1
exec "${real_mv}" "\$@"
EOF
printf '#!/bin/sh\nexit 0\n' > "${stubs}/corepack"
printf '#!/bin/sh\necho "${HOST_NODE:-v20.0.0}"\n' > "${stubs}/node"
chmod +x "${stubs}"/*

work="${tmp}/work"
mkdir -p "${work}"
echo "${pin}" > "${work}/.node-version"
cache="${tmp}/cache"
pin_dir="${cache}/${name}"

# run <script>: the script in a shell with the four functions, in a checkout
# with .node-version, the stubs first on PATH and the test cache.
run() {
  (cd "${work}" && CI_NODE_CACHE="${cache}" PATH="${stubs}:${PATH}" bash -c "
    set -uo pipefail
    ${defs}
    $1
  ")
}
curl_calls() { [ -f "${tmp}/curl.log" ] && wc -l < "${tmp}/curl.log" || echo 0; }

# Activation never downloads, and refuses a missing or wrong kept copy.
rm -f "${tmp}/curl.log"
out="$(run 'activate_pinned_node; echo "status=$?"; echo "path=${PATH}"' 2>&1)"
[[ "${out}" == *"status=1"* ]] && [[ "${out}" != *"${pin_dir}/bin"* ]] && [ "$(curl_calls)" -eq 0 ] &&
  pass "activation refuses a missing kept copy, without downloading or changing PATH" ||
  fail "activation with no kept copy" "${out}"
mkdir -p "${pin_dir}/bin"
printf '#!/bin/sh\necho v0.0.1\n' > "${pin_dir}/bin/node"
chmod +x "${pin_dir}/bin/node"
out="$(run 'activate_pinned_node; echo "status=$?"' 2>&1)"
[[ "${out}" == *"status=1"* ]] && [ "$(curl_calls)" -eq 0 ] &&
  pass "activation refuses a kept copy of another version" ||
  fail "activation with a wrong kept copy" "${out}"
rm -rf "${cache}"

# A copy that can't be kept fails the fetch, rather than the host's Node
# standing in for it later.
out="$(FAIL_MV=1 run 'fetch_pinned_node; echo "status=$?"' 2>&1)"
[[ "${out}" == *"status=1"* ]] && [[ "${out}" == *"Couldn't keep"* ]] && [ ! -e "${pin_dir}" ] &&
  pass "a copy that can't be kept fails the fetch" ||
  fail "fetch when the copy can't be kept" "${out}"
out="$(run 'activate_pinned_node; echo "status=$?"' 2>&1)"
[[ "${out}" == *"status=1"* ]] &&
  pass "  and activation then refuses too" || fail "activation after a failed keep" "${out}"

# A good fetch keeps the checked copy; activation then puts it first on PATH.
rm -f "${tmp}/curl.log"
out="$(run 'fetch_pinned_node; echo "status=$?"' 2>&1)"
[[ "${out}" == *"status=0"* ]] && [ "$("${pin_dir}/bin/node" --version)" = "v${pin}" ] &&
  [ "$(curl_calls)" -eq 2 ] &&
  pass "a fetch checks the archive and keeps a copy of exactly the pinned version" ||
  fail "fetch" "${out}"
rm -f "${tmp}/curl.log"
out="$(run 'activate_pinned_node > /dev/null; echo "status=$?"; echo "node=$(command -v node)"; echo "version=$(node --version)"' 2>&1)"
[[ "${out}" == *"status=0"* ]] && [[ "${out}" == *"node=${pin_dir}/bin/node"* ]] &&
  [[ "${out}" == *"version=v${pin}"* ]] && [ "$(curl_calls)" -eq 0 ] &&
  pass "activation puts the kept copy first on PATH, without downloading" ||
  fail "activation with a kept copy" "${out}"
out="$(run 'fetch_pinned_node; echo "status=$?"' 2>&1)"
[[ "${out}" == *"status=0"* ]] && [ "$(curl_calls)" -eq 0 ] &&
  pass "a second fetch uses the kept copy" || fail "fetch with a kept copy" "${out}"

# A broken kept copy is replaced by a fresh, checked one.
printf '#!/bin/sh\nexit 1\n' > "${pin_dir}/bin/node"
out="$(run 'fetch_pinned_node; echo "status=$?"' 2>&1)"
[[ "${out}" == *"status=0"* ]] && [ "$("${pin_dir}/bin/node" --version)" = "v${pin}" ] &&
  pass "a broken kept copy is replaced" || fail "fetch over a broken copy" "${out}"

# An archive that doesn't match its checksum is refused.
rm -rf "${cache}"
cp "${fixtures}/SHASUMS256.txt" "${tmp}/SHASUMS256.good"
# Always another first hex digit: 0 becomes 1, anything else 0.
sed -i -E 's/^0/x/; s/^[1-9a-f]/0/; s/^x/1/' "${fixtures}/SHASUMS256.txt"
if cmp -s "${tmp}/SHASUMS256.good" "${fixtures}/SHASUMS256.txt"; then
  fail "checksum" "the fixture's SHA-256 list didn't change"
fi
out="$(run 'fetch_pinned_node; echo "status=$?"' 2>&1)"
cp "${tmp}/SHASUMS256.good" "${fixtures}/SHASUMS256.txt"
[[ "${out}" == *"status=1"* ]] && [ ! -e "${pin_dir}" ] &&
  pass "an archive that doesn't match its checksum is refused" || fail "checksum" "${out}"

# A host Node of exactly the pinned version is used as it is.
rm -f "${tmp}/curl.log"
out="$(HOST_NODE="v${pin}" run 'fetch_pinned_node && activate_pinned_node; echo "status=$?"; echo "path=${PATH}"' 2>&1)"
[[ "${out}" == *"status=0"* ]] && [[ "${out}" != *"${pin_dir}/bin"* ]] && [ "$(curl_calls)" -eq 0 ] &&
  pass "a host Node of the pinned version is used as it is" || fail "host Node" "${out}"

if [ "${failures}" -gt 0 ]; then
  echo "${failures} failed"
  exit 1
fi
echo "All pinned Node checks passed."
