#!/usr/bin/bash
# Shared helpers for agent-workspace-backup, agent-workspace-backup-key and
# agent-workspace-restore-drill. Source it; do not run it.
#
# Nothing here names a machine. Every host, path and recipient comes from the
# backup config file (see backup.env.example).

export PATH=/usr/local/sbin:/usr/local/bin:/usr/sbin:/usr/bin:/sbin:/bin
export LC_ALL=C

# shellcheck disable=SC2034 # used by the scripts that source this file
readonly BACKUP_DEFAULT_CONFIG=/etc/agent-workspace/backup.env
# A backup set is a directory named by its UTC start time.
readonly SET_NAME_PATTERN='^[0-9]{8}T[0-9]{6}Z$'
# Remote paths and hosts go into rsync and ssh arguments; keep them plain.
readonly SAFE_PATH_PATTERN='^[A-Za-z0-9._/~-]+$'
readonly SAFE_HOST_PATTERN='^([A-Za-z0-9._-]+@)?[A-Za-z0-9._-]+$'

log() {
  printf '%s %s\n' "$(date -u +%Y-%m-%dT%H:%M:%SZ)" "$*" >&2
}

die() {
  log "error: $*"
  exit 1
}

# Read KEY=VALUE lines into shell variables without executing anything.
# Only BACKUP_* keys are accepted. A variable already set in the environment
# wins over the file, so one-off runs can override a single setting.
load_config() {
  local file="$1" line key value number=0
  [[ -f "${file}" ]] || die "config file ${file} not found"
  while IFS= read -r line || [[ -n "${line}" ]]; do
    number=$((number + 1))
    [[ "${line}" =~ ^[[:space:]]*(#.*)?$ ]] && continue
    if [[ ! "${line}" =~ ^(BACKUP_[A-Z0-9_]+)=(.*)$ ]]; then
      die "${file}:${number}: expected BACKUP_NAME=value"
    fi
    key="${BASH_REMATCH[1]}"
    value="${BASH_REMATCH[2]}"
    if [[ "${value}" =~ ^\"(.*)\"$ || "${value}" =~ ^\'(.*)\'$ ]]; then
      value="${BASH_REMATCH[1]}"
    fi
    if [[ -z "${!key+x}" ]]; then
      printf -v "${key}" '%s' "${value}"
    fi
  done <"${file}"
}

# Defaults for settings every script shares.
apply_common_defaults() {
  : "${BACKUP_DATABASE:=agent_workspace}"
  : "${BACKUP_MAINTENANCE_DATABASE:=postgres}"
  : "${BACKUP_RELEASE_BIN:=/opt/agent-workspace/current/bin}"
  : "${BACKUP_PG_DUMP:=${BACKUP_RELEASE_BIN}/agent-workspace-pg-dump}"
  : "${BACKUP_PG_RESTORE:=${BACKUP_RELEASE_BIN}/agent-workspace-pg-restore}"
  : "${BACKUP_PSQL:=${BACKUP_RELEASE_BIN}/agent-workspace-psql}"
  : "${BACKUP_LOCAL_DIR:=/var/backups/agent-workspace/encrypted}"
  : "${BACKUP_STATE_DIR:=/var/lib/agent-workspace-backup}"
  : "${BACKUP_WORK_DIR:=/var/tmp}"
  : "${BACKUP_SSH_PORT:=22}"
  : "${BACKUP_SSH_IDENTITY:=}"
  : "${BACKUP_SSH_KNOWN_HOSTS:=}"
  validate_identifier BACKUP_DATABASE "${BACKUP_DATABASE}"
  validate_identifier BACKUP_MAINTENANCE_DATABASE "${BACKUP_MAINTENANCE_DATABASE}"
  [[ "${BACKUP_SSH_PORT}" =~ ^[0-9]{1,5}$ ]] || die "BACKUP_SSH_PORT must be a number"
}

require_setting() {
  local name="$1"
  [[ -n "${!name:-}" ]] || die "${name} is not set in the backup config"
}

validate_identifier() {
  local name="$1" value="$2"
  [[ "${value}" =~ ^[a-z_][a-z0-9_]{0,62}$ ]] \
    || die "${name} must be a plain lower-case PostgreSQL name"
}

validate_count() {
  local name="$1" value="$2"
  [[ "${value}" =~ ^[0-9]+$ ]] || die "${name} must be a whole number"
}

require_file() {
  local name="$1" path="$2"
  [[ "${path}" == /* ]] || die "${name} must be an absolute path"
  [[ -f "${path}" && -r "${path}" ]] || die "${name} (${path}) is not a readable file"
}

require_tools() {
  local tool
  for tool in "$@"; do
    command -v "${tool}" >/dev/null 2>&1 || die "required tool ${tool} is not installed"
  done
}

new_set_name() {
  date -u +%Y%m%dT%H%M%SZ
}

# Set names older than this cut-off are eligible for pruning.
cutoff_name() {
  local days="$1"
  date -u -d "-${days} days" +%Y%m%dT%H%M%SZ
}

# Hold an exclusive lock for the rest of the process.
take_lock() {
  local name="$1"
  install -d -m 0700 "${BACKUP_STATE_DIR}"
  exec 9>"${BACKUP_STATE_DIR}/${name}.lock"
  flock -n 9 || die "another ${name} run holds ${BACKUP_STATE_DIR}/${name}.lock"
}

# age encryption: recipients are public keys, so the host never needs the
# private identity to make a backup.
encrypt_stream() {
  local recipients_file="$1" output="$2"
  age --encrypt --recipients-file "${recipients_file}" --output "${output}"
}

decrypt_stream() {
  local identity_file="$1" input="$2"
  age --decrypt --identity "${identity_file}" "${input}"
}

# ---------------------------------------------------------------------------
# Off-host transport. Every copy goes through the five remote_* functions
# below, so adding another transport (for example rclone) means one more case
# in each of them.
#
# A remote is written as one of:
#   ssh:[user@]host:/absolute/path   rsync over ssh (the default transport)
#   local:/absolute/path             a directory on this machine (tests, or a
#                                    mounted disk that leaves with you)
# ---------------------------------------------------------------------------

remote_kind=""
remote_host=""
remote_path=""

parse_remote() {
  local spec="$1" rest
  case "${spec}" in
    ssh:*)
      remote_kind=ssh
      rest="${spec#ssh:}"
      remote_host="${rest%%:*}"
      remote_path="${rest#*:}"
      [[ "${rest}" == *:* ]] || die "ssh remote must look like ssh:user@host:/path"
      [[ "${remote_host}" =~ ${SAFE_HOST_PATTERN} ]] || die "remote host has unexpected characters"
      ;;
    local:*)
      remote_kind=local
      remote_host=""
      remote_path="${spec#local:}"
      ;;
    *)
      die "remote must start with ssh: or local:"
      ;;
  esac
  [[ "${remote_path}" == /* ]] || die "remote path must be absolute"
  [[ "${remote_path}" =~ ${SAFE_PATH_PATTERN} ]] || die "remote path has unexpected characters"
  [[ "${remote_path}" != */ ]] || remote_path="${remote_path%/}"
  [[ -n "${remote_path}" && "${remote_path}" != / ]] || die "remote path must not be /"
}

# rsync and ssh are needed only for an ssh: remote.
require_transport_tools() {
  parse_remote "$1"
  if [[ "${remote_kind}" == ssh ]]; then
    require_tools rsync ssh
  fi
}

ssh_command=()
build_ssh_command() {
  ssh_command=(ssh -o BatchMode=yes -o StrictHostKeyChecking=yes -p "${BACKUP_SSH_PORT}")
  if [[ -n "${BACKUP_SSH_IDENTITY}" ]]; then
    require_file BACKUP_SSH_IDENTITY "${BACKUP_SSH_IDENTITY}"
    ssh_command+=(-o IdentitiesOnly=yes -i "${BACKUP_SSH_IDENTITY}")
  fi
  if [[ -n "${BACKUP_SSH_KNOWN_HOSTS}" ]]; then
    require_file BACKUP_SSH_KNOWN_HOSTS "${BACKUP_SSH_KNOWN_HOSTS}"
    ssh_command+=(-o UserKnownHostsFile="${BACKUP_SSH_KNOWN_HOSTS}")
  fi
}

# rsync takes the ssh command as one string; every part was validated above
# and contains no spaces, and %q keeps it that way.
rsync_ssh_string() {
  local part out=""
  for part in "${ssh_command[@]}"; do
    [[ "${part}" != *[[:space:]]* ]] || die "ssh settings must not contain spaces"
    out+="$(printf '%q' "${part}") "
  done
  printf '%s' "${out% }"
}

# Run one fixed command on the remote host. The remote shell parses the line,
# so each word is quoted with %q; set names and paths are validated first.
remote_shell() {
  local quoted="" word
  for word in "$@"; do
    quoted+="$(printf '%q' "${word}") "
  done
  "${ssh_command[@]}" -- "${remote_host}" "${quoted% }"
}

# Print the complete set names held by the remote, oldest first.
remote_list() {
  local spec="$1" listing=""
  parse_remote "${spec}"
  case "${remote_kind}" in
    local)
      [[ -d "${remote_path}" ]] || return 0
      listing="$(find "${remote_path}" -mindepth 1 -maxdepth 1 -type d -printf '%f\n')"
      ;;
    ssh)
      build_ssh_command
      remote_shell mkdir -p -- "${remote_path}"
      listing="$(remote_shell ls -1 -- "${remote_path}")"
      ;;
  esac
  filter_set_names <<<"${listing}"
}

# Keep only complete set names (not .partial or anything else), oldest first.
filter_set_names() {
  { grep -E "${SET_NAME_PATTERN}" || true; } | sort
}

# Copy a finished local set directory to the remote. It lands under a
# .partial name, is renamed only after every file arrived, and the
# destination's filesystem is synced after the rename.
remote_put_set() {
  local spec="$1" local_set="$2" name
  name="$(basename -- "${local_set}")"
  [[ "${name}" =~ ${SET_NAME_PATTERN} ]] || die "bad set name ${name}"
  parse_remote "${spec}"
  case "${remote_kind}" in
    local)
      install -d -m 0700 "${remote_path}"
      rm -rf -- "${remote_path:?}/${name}.partial"
      cp -a -- "${local_set}" "${remote_path}/${name}.partial"
      mv -T -- "${remote_path}/${name}.partial" "${remote_path}/${name}"
      # Flush the files and the rename before calling the set stored.
      sync -f -- "${remote_path}/${name}"
      ;;
    ssh)
      build_ssh_command
      remote_shell mkdir -p -- "${remote_path}"
      rsync --archive --partial --chmod=D0700,F0600 \
        --rsh="$(rsync_ssh_string)" \
        -- "${local_set}/" "${remote_host}:${remote_path}/${name}.partial/"
      # -T: never move the set inside an existing one of the same name.
      remote_shell mv -T -- "${remote_path}/${name}.partial" "${remote_path}/${name}"
      remote_shell sync -f -- "${remote_path}/${name}"
      ;;
  esac
}

# Fetch one remote set into an empty local directory.
remote_get_set() {
  local spec="$1" name="$2" destination="$3"
  [[ "${name}" =~ ${SET_NAME_PATTERN} ]] || die "bad set name ${name}"
  parse_remote "${spec}"
  case "${remote_kind}" in
    local)
      cp -a -- "${remote_path}/${name}/." "${destination}/"
      ;;
    ssh)
      build_ssh_command
      rsync --archive \
        --rsh="$(rsync_ssh_string)" \
        -- "${remote_host}:${remote_path}/${name}/" "${destination}/"
      ;;
  esac
}

# Whether the remote copy of set $2 is whole and the same as the local set
# in $3: its SHA256SUMS matches the local one and every file in it checks
# out. The remote host does the hashing, so nothing large is transferred.
remote_set_verified() {
  local spec="$1" name="$2" local_set="$3" remote_sums
  [[ "${name}" =~ ${SET_NAME_PATTERN} ]] || die "bad set name ${name}"
  [[ -s "${local_set}/SHA256SUMS" ]] || return 1
  parse_remote "${spec}"
  case "${remote_kind}" in
    local)
      [[ -f "${remote_path}/${name}/SHA256SUMS" ]] || return 1
      cmp -s -- "${local_set}/SHA256SUMS" "${remote_path}/${name}/SHA256SUMS" || return 1
      (cd -- "${remote_path}/${name}" && sha256sum --status -c SHA256SUMS)
      ;;
    ssh)
      build_ssh_command
      remote_sums="$(remote_shell cat -- "${remote_path}/${name}/SHA256SUMS" 2>/dev/null)" || return 1
      [[ "${remote_sums}" == "$(<"${local_set}/SHA256SUMS")" ]] || return 1
      # shellcheck disable=SC2016 # $1 expands on the remote host.
      remote_shell sh -c 'cd -- "$1" && sha256sum --status -c SHA256SUMS' sh \
        "${remote_path}/${name}"
      ;;
  esac
}

remote_delete_set() {
  local spec="$1" name="$2"
  [[ "${name}" =~ ${SET_NAME_PATTERN} ]] || die "bad set name ${name}"
  parse_remote "${spec}"
  case "${remote_kind}" in
    local)
      rm -rf -- "${remote_path:?}/${name}"
      ;;
    ssh)
      build_ssh_command
      remote_shell rm -rf -- "${remote_path}/${name}"
      ;;
  esac
}

# Given set names (oldest first) on stdin, print the ones to delete: older
# than the cut-off, never touching the newest keep_min sets.
select_prunable() {
  local keep_days="$1" keep_min="$2" cutoff
  local -a names=()
  local index
  cutoff="$(cutoff_name "${keep_days}")"
  mapfile -t names
  for ((index = 0; index < ${#names[@]} - keep_min; index++)); do
    if [[ "${names[index]}" < "${cutoff}" ]]; then
      printf '%s\n' "${names[index]}"
    fi
  done
}

# Set names in a local directory, oldest first.
local_list() {
  local dir="$1"
  local listing
  [[ -d "${dir}" ]] || return 0
  listing="$(find "${dir}" -mindepth 1 -maxdepth 1 -type d -printf '%f\n')"
  filter_set_names <<<"${listing}"
}

# The two remotes must never share a place: a dump next to its key defeats
# the encryption.
# The recipients in an age recipients file, one per line and sorted: comment
# and blank lines dropped, and an SSH key reduced to its type and key (age
# ignores the trailing comment). age lets every recipient in a file decrypt
# on its own, so two files are separate only if these sets are.
age_recipients() {
  awk '$1 !~ /^#/ && NF { if ($1 ~ /^ssh-/) print $1, $2; else print $1 }' "$1" \
    | LC_ALL=C sort -u
}

require_separate_recipients() {
  local data_file="$1" key_file="$2" shared
  [[ -n "$(age_recipients "${key_file}")" ]] || die "${key_file} lists no age recipients"
  shared="$(LC_ALL=C comm -12 <(age_recipients "${data_file}") <(age_recipients "${key_file}"))"
  [[ -z "${shared}" ]] \
    || die "BACKUP_KEY_AGE_RECIPIENTS_FILE shares a recipient with BACKUP_AGE_RECIPIENTS_FILE; one identity could open both the database dump and the secrets key"
}

# A destination's path in a comparable form: a local one fully resolved
# (symlinks too), a remote one with ., .. and repeated slashes folded
# (symlinks on the remote host can't be seen from here).
# The two backups must never share a directory: database retention would
# prune key sets, which are kept forever.
comparable_remote_path() {
  case "${remote_kind}" in
    local) realpath -m -- "${remote_path}" ;;
    ssh) realpath -m -s -- "${remote_path}" ;;
  esac
}

require_separate_remotes() {
  local data_spec="$1" key_spec="$2" data_kind data_host data_path key_path
  [[ "${data_spec}" != "${key_spec}" ]] || die "BACKUP_KEY_REMOTE must differ from BACKUP_REMOTE"
  parse_remote "${data_spec}"
  data_kind="${remote_kind}"
  data_host="${remote_host#*@}"
  data_path="$(comparable_remote_path)"
  parse_remote "${key_spec}"
  key_path="$(comparable_remote_path)"
  # The login part doesn't change which machine it is.
  if [[ "${remote_kind}" == "${data_kind}" && "${remote_host#*@}" == "${data_host}" ]]; then
    if [[ "${key_path%/}/" == "${data_path%/}/"* || "${data_path%/}/" == "${key_path%/}/"* ]]; then
      die "BACKUP_KEY_REMOTE must not be inside BACKUP_REMOTE or contain it"
    fi
  fi
}
