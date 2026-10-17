/** Audit action codes in plain words. Unknown codes are shown as they are. */
const PHRASES: Record<string, string> = {
  'authentication.login': 'signed in',
  'authentication.logout': 'signed out',
  'authentication.two_factor': 'signed in with a two-factor code',
  'authentication.workos': 'signed in with single sign-on',
  'account.password.change': 'changed their password',
  'account.session.revoke': 'signed out a session',
  'account.session.revoke_others': 'signed out their other sessions',
  'account.two_factor.enable': 'turned on two-factor authentication',
  'account.two_factor.disable': 'turned off two-factor authentication',
  'account.two_factor.recovery_codes': 'made new recovery codes',
  'setting.changed': 'changed a setting',
  'setting.reset': 'reset a setting',
  'secret.saved': 'saved a key',
  'secret.deleted': 'deleted a key',
  'workspace.renamed': 'renamed the workspace',
  'workspace.ownership_transferred': 'transferred ownership of the workspace',
  'workspace.invitation.issue': 'invited someone to the workspace',
  'workspace.invitation.revoke': 'revoked a workspace invitation',
  'workspace_membership.role_changed': 'changed a workspace role',
  'workspace_membership.removed': 'removed someone from the workspace',
  'workspace_membership.left': 'left the workspace',
  'projects.create': 'created a Project',
  'project.created': 'created a Project',
  'project.archived': 'archived a Project',
  'project.mcp_archived': 'archived a Project',
  'project.mcp_restored': 'restored a Project',
  'project.mcp_renamed': 'renamed a Project',
  'project.invitation.issue': 'invited someone to a Project',
  'project.invitation.replace': 'replaced a Project invitation link',
  'project.invitation.revoke': 'revoked a Project invitation',
  'project.membership.updated': 'changed a Project membership',
  'project_membership.updated': 'changed a Project membership',
  'project_membership.mcp_added': 'added someone to a Project',
  'project_membership.mcp_changed': 'changed a Project membership',
  'project_membership.mcp_removed': 'removed someone from a Project',
  'mcp.credential.create': 'created an API token',
  'mcp.credential.replace': 'replaced an API token',
  'mcp.credential.revoke': 'revoked an API token',
  'external_agent.created': 'connected an agent',
  'external_agent.updated': 'changed an agent',
  'external_agent.disabled': 'turned off an agent',
  'external_agent.removed': 'removed an agent',
  'external_agent.credential_rotated': 'replaced an agent’s credential',
  'external_agent.webhook_updated': 'changed an agent’s webhook',
  'repository.write': 'changed a repository',
  'work_item.created': 'created a work item',
  'work_item.mcp_created': 'created a work item',
  'work_item.updated': 'changed a work item',
  'work_item.mcp_updated': 'changed a work item',
  'work_item.changed': 'changed a work item',
  'work_item.moved': 'moved a work item',
  'work_item.mcp_status_updated': 'moved a work item',
  'work_item.assigned': 'assigned a work item',
  'work_item.archived': 'archived a work item',
  'work_item.mcp_archived': 'archived a work item',
  'work_item.restored': 'restored a work item',
  'work_item.comment_created': 'commented on a work item',
  'conversation.created': 'started a conversation',
  'conversation.channel_created': 'created a channel',
  'conversation.thread_created': 'started a thread',
  'conversation.archived': 'archived a conversation',
  'conversation.message_edited': 'edited a message',
  'conversation.message_removed': 'removed a message',
}

/** "changed a setting", or null for a code there are no words for yet. */
export function actionPhrase(action: string): string | null {
  return PHRASES[action] ?? null
}

/** "Signed in": an action on its own, starting with a capital. Falls back to the code. */
export function actionSentence(action: string, fallback: string | null = action) {
  const phrase = actionPhrase(action)
  if (!phrase) return fallback ?? action
  return phrase.charAt(0).toUpperCase() + phrase.slice(1)
}

/** The thing the action was about, from its metadata, such as a setting's key or a key's name. */
export function actionDetail(action: string, metadata: Record<string, unknown>): string | null {
  const pick = (...names: string[]) => {
    for (const name of names) {
      const value = metadata[name]
      if (typeof value === 'string' && value) return value
    }
    return null
  }
  if (action.startsWith('setting.')) return pick('key')
  if (action.startsWith('secret.')) return pick('name')
  if (action === 'workspace.renamed') return pick('name', 'newName')
  if (action.startsWith('workspace_membership.')) return pick('role', 'displayName')
  return pick('name', 'title', 'displayName')
}
