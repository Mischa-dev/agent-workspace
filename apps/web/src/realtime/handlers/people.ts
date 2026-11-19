import { agentKeys } from '../../agents/keys'
import { queryKeys } from '../../api/queryKeys'
import { direct, inbox, opAnswers, payloadId, rail, type Handler } from './touches'

// Membership, notifications and account managers.

/**
 * Someone joined, left or changed role in a space. What anyone may see or do
 * there can change, so this is the one event that refetches the whole space
 * (like K5's `access` frame), and the lists of spaces.
 */
const membershipUpdated: Handler = (event) => [
  { id: `space:${event.projectId}`, filters: { queryKey: queryKeys.project(event.projectId) } },
  { id: 'projects', filters: { queryKey: queryKeys.projectLists } },
  direct,
]

/** The viewer's notifications changed: the inbox and the rail's attention counts. */
const notificationsUpdated: Handler = () => [inbox, rail]

/** Who manages an agent or app account changed: the workspace's settings and the space's agents. */
const managersChanged: Handler = (event) => {
  const workspaceId = payloadId(event, 'workspaceId', 'workspace_id')
  return [
    ...(workspaceId ? [{ id: `workspace:${workspaceId}`, filters: { queryKey: queryKeys.organization(workspaceId) } }] : []),
    { id: `agent-profiles:${event.projectId}`, filters: { queryKey: agentKeys.profiles(event.projectId) } },
    { id: `agents:${event.projectId}`, filters: { queryKey: agentKeys.presence(event.projectId) } },
  ]
}

/**
 * A workspace was suspended, resumed, scheduled for deletion or kept (H1.24):
 * the session (each workspace's lifecycle) and the workspace's Settings ›
 * General, exactly.
 */
const lifecycleChanged: Handler = (event) => {
  const workspaceId = payloadId(event, 'workspaceId', 'workspace_id')
  return [
    { id: 'session', filters: { queryKey: queryKeys.session, exact: true } },
    ...(workspaceId
      ? [{ id: `workspace-settings:${workspaceId}`, filters: { queryKey: queryKeys.organization(workspaceId), exact: true } }]
      : []),
  ]
}

/** A workspace's own emoji changed: its pickers read the list again. */
const customEmojiChanged: Handler = (event) => {
  const workspaceId = payloadId(event, 'workspaceId', 'workspace_id')
  return [
    workspaceId
      ? { id: `custom-emoji:${workspaceId}`, filters: { queryKey: queryKeys.customEmojiOf(workspaceId) } }
      : { id: 'custom-emoji', filters: { queryKey: queryKeys.customEmoji } },
  ]
}

/** One of your own lists changed on another device: that operation's queries. */
const ownList =
  (op: string): Handler =>
  () => [opAnswers(op)]

/** A block, or a message request answered (C2.39): DM standings, blocks and the DM list. */
const dmRulesChanged: Handler = () => [
  opAnswers('dm_standing_get'),
  opAnswers('account_block_list'),
  opAnswers('sidebar_list'),
  direct,
]

/** Someone's profile or avatar changed: profiles and avatars read again. */
const profileChanged: Handler = () => [opAnswers('profile_get'), opAnswers('profile_avatar_list')]

/** A theme you made, or one the workspace offers, changed: the theme lists and that theme. */
const themesChanged: Handler = (event) => {
  const themeId = payloadId(event, 'themeId', 'theme_id')
  const named = (query: { queryKey: readonly unknown[] }) => (query.queryKey[3] as { id?: unknown } | undefined)?.id === themeId
  return [
    { id: 'op:theme_list', filters: { predicate: (query) => query.queryKey[0] === 'op' && query.queryKey[2] === 'theme_list' } },
    ...(themeId
      ? [{ id: `theme:${themeId}`, filters: { predicate: (query: { queryKey: readonly unknown[] }) => query.queryKey[0] === 'op' && query.queryKey[2] === 'theme_get' && named(query) } }]
      : []),
  ]
}

/** Someone's status changed: the names that show it read it again. */
const statusChanged: Handler = () => [{ id: 'statuses', filters: { queryKey: queryKeys.statuses } }]

export const peopleHandlers: Record<string, Handler> = {
  'project.membership.updated': membershipUpdated,
  'notifications.updated': notificationsUpdated,
  'account.managers.changed': managersChanged,
  'workspace.lifecycle.changed': lifecycleChanged,
  'custom_emoji.changed': customEmojiChanged,
  'theme.changed': themesChanged,
  'workspace_themes.changed': themesChanged,
  'message_draft.changed': ownList('message_draft_list'),
  'notification_keywords.changed': ownList('notification_keyword_list'),
  'sidebar.changed': ownList('sidebar_list'),
  'saved_searches.changed': ownList('saved_search_list'),
  'inbox_views.changed': ownList('inbox_view_list'),
  'follows.changed': ownList('follow_list'),
  // A team's guards changed (A2.27): the lists of them read again.
  'guards.changed': ownList('guard_list'),
  'dm_rules.changed': dmRulesChanged,
  'account.profile_changed': profileChanged,
  'account.status.changed': statusChanged,
}
