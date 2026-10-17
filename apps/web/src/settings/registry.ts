// Every settings page, one line each. To add a page, add a line in its group
// (Account, Workspace or Project) next to the related ones and write the page
// as a default-exported component taking SettingsPageProps. Add lines; don't
// reorder: other branches add theirs (environments, notifications and alert
// rules, AI and models) at the same time.
import { lazy, type ComponentType, type LazyExoticComponent } from 'react'

import type { IconName } from '../ui'
import type { SettingsGroup, SettingsPageProps, SettingsVisibility } from './types'

export type SettingsPage = {
  group: SettingsGroup
  /** The last part of the URL, such as `security` in /settings/account/security. */
  id: string
  label: string
  icon: IconName
  /** Other words people search for. */
  keywords: string[]
  /** Registry settings shown on this page, by key prefix (such as `run.`), so search can find them. */
  settings?: string[]
  component: LazyExoticComponent<ComponentType<SettingsPageProps>>
  /** Leave it out of the nav when the person can't use it. */
  visible?: (context: SettingsVisibility) => boolean
}

type PageModule = { default: ComponentType<SettingsPageProps> }

function page(
  group: SettingsGroup,
  id: string,
  label: string,
  icon: IconName,
  keywords: string[],
  load: () => Promise<PageModule>,
  options: Pick<SettingsPage, 'settings' | 'visible'> = {},
): SettingsPage {
  return { group, id, label, icon, keywords, component: lazy(load), ...options }
}

const manager = ({ workspace }: SettingsVisibility) => workspace?.role === 'owner' || workspace?.role === 'admin'
const manageProject = ({ project }: SettingsVisibility) => project?.capabilities.manage === true
const projectMember = ({ project }: SettingsVisibility) => Boolean(project) && project?.role !== 'viewer'
const runsSomewhere = ({ projects }: SettingsVisibility) => projects.some((project) => project.role !== 'viewer')

export const SETTINGS_PAGES: SettingsPage[] = [
  // Account
  page('account', 'profile', 'Profile', 'at-sign', ['name', 'display name', 'username', 'github username', 'workos', 'google', 'sign-in'], () => import('./account/ProfilePage')),
  page('account', 'security', 'Security', 'shield-check', ['password', 'sessions', 'devices', 'sign out', 'two-factor', '2fa', 'authenticator', 'recovery codes'], () => import('./account/SecurityPage')),
  page('account', 'appearance', 'Appearance', 'palette', ['theme', 'dark', 'light', 'colors', 'density', 'text size'], () => import('../theme/AppearancePage')),
  page('account', 'provider-accounts', 'Provider accounts', 'terminal', ['codex', 'sign in', 'chatgpt', 'runs'], () => import('./account/ProviderAccountsPage'), { visible: runsSomewhere }),
  page('account', 'usage', 'Usage and alerts', 'activity', ['limits', 'rate limits', 'windows', 'resets', 'capacity', 'quota', 'banked reset', 'alerts'], () => import('./account/UsagePage'), { visible: runsSomewhere }),
  page('account', 'notifications', 'Notifications', 'inbox', ['push', 'phone', 'desktop', 'quiet hours', 'mute', 'email', 'digest', 'do not disturb'], () => import('./account/NotificationsPage'), { settings: ['notifications.'] }),
  page('account', 'connected-apps', 'Connected apps', 'link', ['mcp', 'claude code', 'oauth', 'apps'], () => import('./account/ConnectedAppsPage')),
  page('account', 'api-tokens', 'API tokens', 'key', ['token', 'bearer', 'mcp', 'credential', 'scripts'], () => import('./account/ApiTokensPage')),
  // Workspace
  page('workspace', 'general', 'General', 'settings', ['rename', 'workspace name', 'leave', 'transfer ownership', 'owner'], () => import('./workspace/GeneralPage')),
  page('workspace', 'members', 'Members', 'users', ['people', 'agents', 'roles', 'admin', 'invite', 'invitations', 'remove'], () => import('./workspace/MembersPage')),
  page('workspace', 'ai', 'AI and models', 'bot', ['provider', 'model', 'suggestions', 'api key', 'budget', 'usage', 'anthropic', 'openai', 'gemini'], () => import('./workspace/AiPage'), { settings: ['ai.'] }),
  page('workspace', 'keys', 'Keys', 'key', ['secrets', 'api keys', 'credentials'], () => import('./workspace/KeysPage'), { visible: manager }),
  page('workspace', 'run-defaults', 'Environments and run defaults', 'settings', ['harness', 'model', 'reasoning', 'speed', 'environment'], () => import('./workspace/RunDefaultsPage'), { settings: ['run.'] }),
  page('workspace', 'harnesses', 'Harnesses', 'terminal', ['custom harness', 'adapter', 'manifest', 'runner', 'command', 'agents'], () => import('./workspace/HarnessesPage')),
  page('workspace', 'project-templates', 'Project templates', 'list', ['templates', 'new project', 'setup', 'starter'], () => import('./workspace/ProjectTemplatesPage')),
  page('workspace', 'github', 'GitHub', 'pull-request', ['installation', 'app', 'repositories'], () => import('./workspace/GitHubPage'), { visible: manager }),
  page('workspace', 'audit-log', 'Audit log', 'activity', ['history', 'activity', 'who changed', 'events'], () => import('./workspace/AuditLogPage'), { visible: manager }),
  // Project
  page('project', 'general', 'General', 'settings', ['about', 'archive', 'key', 'name'], () => import('./project/GeneralPage')),
  page('project', 'members', 'Members', 'users', ['team', 'people', 'roles', 'invite'], () => import('./project/MembersPage'), {
    visible: (context) => manageProject(context) && Boolean(context.workspace),
  }),
  page('project', 'repositories', 'Repositories', 'repo', ['github', 'connect', 'mirror', 'code'], () => import('./project/RepositoriesPage')),
  page('project', 'environments', 'Environments and defaults', 'settings', ['harness', 'model', 'reasoning', 'speed', 'repository'], () => import('./project/EnvironmentsPage'), { settings: ['run.'] }),
  page('project', 'ai', 'AI and models', 'bot', ['provider', 'model', 'suggestions', 'dependencies', 'automatic'], () => import('./project/AiPage'), { settings: ['ai.'] }),
  page('project', 'agents', 'Agents', 'bot', ['external agents', 'connections', 'credentials', 'profiles', 'reviewer', 'planner', 'fixer', 'instructions'], () => import('./project/AgentsPage'), { visible: manageProject }),
  page('project', 'automation', 'Automation', 'checklist', ['pull requests', 'work items', 'review', 'github'], () => import('./project/AutomationPage'), { visible: manageProject }),
  page('project', 'workflow', 'Workflow', 'work', ['statuses', 'types', 'estimates', 'transitions', 'board', 'columns'], () => import('./project/WorkflowPage'), { visible: projectMember }),
  page('project', 'templates', 'Templates', 'list', ['task templates', 'recurring', 'repeat', 'schedule', 'project template'], () => import('./project/TemplatesPage'), { visible: projectMember }),
  page('project', 'import-export', 'Import and export', 'arrow-down', ['import', 'export', 'csv', 'markdown', 'github issues', 'linear', 'jira', 'json', 'download'], () => import('./project/ImportExportPage')),
  page('project', 'codex-runs', 'Run requests', 'play', ['codex', 'run requests', 'sponsored', 'approve'], () => import('./project/CodexRunsPage'), { visible: projectMember }),
]

export const GROUP_LABELS: Record<SettingsGroup, string> = {
  account: 'Account',
  workspace: 'Workspace',
  project: 'Project',
}

export function pagesIn(group: SettingsGroup, context: SettingsVisibility) {
  return SETTINGS_PAGES.filter((entry) => entry.group === group && (entry.visible?.(context) ?? true))
}

export function findPage(group: SettingsGroup, id: string) {
  return SETTINGS_PAGES.find((entry) => entry.group === group && entry.id === id) ?? null
}
