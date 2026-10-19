/**
 * What the server's GitHub codes mean for the person looking at a run, each
 * as a sentence that says what to do next. Codes this list doesn't know get
 * a general sentence, never the code itself.
 */

const ACCESS = 'GitHub refused access to the repository. Reconnect GitHub in settings, then try again.'
const NOT_CONNECTED = 'GitHub isn’t connected to this project. Connect it in the project’s settings, then try again.'
const WRONG_REPOSITORY = 'The repository on GitHub isn’t the one this project is linked to. Check the repository in the project’s settings.'

const PUBLISH: Record<string, string> = {
  github_not_configured: NOT_CONNECTED,
  github_token_unavailable: ACCESS,
  github_scope_incompatible: ACCESS,
  installation_identity_invalid: ACCESS,
  publication_scope_unavailable: ACCESS,
  repository_identity_invalid: WRONG_REPOSITORY,
  git_push_failed: 'GitHub refused the push. Publish again; if it keeps failing, check the repository’s branch rules.',
  remote_branch_diverged: 'The branch on GitHub has changes this run doesn’t have. Run again from the latest code.',
  remote_ref_unavailable: 'GitHub didn’t confirm the branch. Publish again.',
  remote_ref_unconfirmed: 'GitHub didn’t confirm the branch. Publish again.',
  workspace_changed: 'The run’s workspace changed after it finished, so it can’t be published. Run again to publish.',
  workspace_unavailable: 'The run’s workspace is gone, so there’s nothing to publish. Run again to publish.',
  runtime_path_mismatch: 'The run’s workspace is gone, so there’s nothing to publish. Run again to publish.',
}

const PULL_REQUEST: Record<string, string> = {
  github_not_configured: NOT_CONNECTED,
  github_token_unavailable: ACCESS,
  github_scope_incompatible: ACCESS,
  installation_identity_invalid: ACCESS,
  pull_request_scope_unavailable: ACCESS,
  repository_identity_invalid: WRONG_REPOSITORY,
  create_outcome_uncertain: 'GitHub didn’t say whether the pull request was opened. Refresh GitHub status to check.',
  duplicate_open_pull_requests: 'This branch already has an open pull request on GitHub.',
  pull_request_lookup_failed: 'GitHub didn’t find the pull request. Refresh GitHub status to check again.',
}

const AUTOMATION_SKIPPED: Record<string, string> = {
  pull_request_draft: 'the pull request is a draft',
  pull_request_closed: 'the pull request was closed without merging',
  no_explicit_work_items: 'no tasks are linked to it',
  rule_disabled: 'moving tasks is turned off for this project',
  terminal_or_archived: 'its tasks are already done or archived',
  ambiguous_target_status: 'more than one status could come next',
  already_in_target_status: 'its tasks are already where they’d move to',
  transition_not_allowed: 'the workflow doesn’t allow that move',
  no_eligible_transition: 'none of its tasks could move',
}

export function publishFailure(category: string) {
  return PUBLISH[category] ?? 'Publishing didn’t finish. Publish again.'
}

export function pullRequestFailure(category: string) {
  return PULL_REQUEST[category] ?? 'The pull request wasn’t opened. Try again.'
}

/** While a pull request is being opened; nothing once it's there or failed. */
export function pullRequestProgress(state: string) {
  return state === 'queued' || state === 'running' ? 'Opening the pull request on GitHub…' : null
}

export function syncFailure() {
  return 'GitHub status didn’t refresh. Refresh GitHub status to try again.'
}

/** What happened to the linked tasks when the pull request changed, if anything worth saying. */
export function automationNote(state: string, reason: string | null) {
  if (state === 'applied') return 'Linked tasks moved to match the pull request.'
  if (state === 'skipped') return `Linked tasks didn’t move: ${AUTOMATION_SKIPPED[reason ?? ''] ?? 'nothing needed to change'}.`
  return null
}
