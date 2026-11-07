// Every settings page, one line each. To add a page, add a line in its group
// (Account, Workspace or Project) next to the related ones and write the page
// as a default-exported component taking SettingsPageProps. Add lines; don't
// reorder: other branches add theirs (notifications and alert rules,
// for example) at the same time. Give each page its own icon; a page with the
// same name in two groups (General, Members, Runs and models) shares one. A renamed
// page's old id redirects in ./route.ts.
import { lazy, type ComponentType, type LazyExoticComponent } from 'react'
import { ArrowDownUp, Bell, Blocks, BookOpen, Bug, Cpu, LayoutTemplate, Mail, PanelsTopLeft, Plug, ScrollText, Server, User, UserCog, Webhook, Zap } from 'lucide-react'
import settingsClaims from 'virtual:modules/slots/settings'

import type { PartId } from '../experience/parts'
import { ownerOf } from '../modules/ownership'
import type { IconGlyph } from '../ui'
import type { SettingsGroup, SettingsPageProps, SettingsVisibility } from './types'

export type SettingsPage = {
  group: SettingsGroup
  /** The last part of the URL, such as `security` in /settings/account/security. */
  id: string
  label: string
  /** A kit icon name, or a lucide icon imported here so it loads with Settings, not with every page. */
  icon: IconGlyph
  /** Other words people search for. */
  keywords: string[]
  /** Registry settings shown on this page, by key prefix (such as `run.`), so search can find them. */
  settings?: string[]
  component: LazyExoticComponent<ComponentType<SettingsPageProps>>
  /** Leave it out of the nav when the person can't use it. */
  visible?: (context: SettingsVisibility) => boolean
  /** The layout part it belongs to: a layout that hides the part leaves the page out of the nav (AWD-66). */
  part?: PartId
}

type PageModule = { default: ComponentType<SettingsPageProps> }

function page(
  group: SettingsGroup,
  id: string,
  label: string,
  icon: IconGlyph,
  keywords: string[],
  load: () => Promise<PageModule>,
  options: Pick<SettingsPage, 'settings' | 'visible' | 'part'> = {},
): SettingsPage {
  return { group, id, label, icon, keywords, component: lazy(load), ...options }
}

const manager = ({ workspace }: SettingsVisibility) => workspace?.role === 'owner' || workspace?.role === 'admin'
const owner = ({ workspace }: SettingsVisibility) => workspace?.role === 'owner'
/** Owns this workspace and runs the server it's on. */
const serverOperator = (context: SettingsVisibility) => owner(context) && context.session.user.capabilities.serverStatus === true
const manageProject = ({ project }: SettingsVisibility) => project?.capabilities.manage === true
const projectMember = ({ project }: SettingsVisibility) => Boolean(project) && project?.role !== 'viewer'
const runsSomewhere = ({ projects }: SettingsVisibility) => projects.some((project) => project.role !== 'viewer')

export const SETTINGS_PAGES: SettingsPage[] = [
  // Account
  page('account', 'profile', 'Profile', User, ['name', 'display name', 'username', 'github username', 'single sign-on', 'sso', 'google', 'sign-in', 'email', 'email address'], () => import('./account/ProfilePage')),
  page('account', 'security', 'Security', 'shield-check', ['password', 'sessions', 'devices', 'sign out', 'two-factor', '2fa', 'authenticator', 'recovery codes'], () => import('./account/SecurityPage')),
  page('account', 'appearance', 'Appearance', 'palette', ['theme', 'dark', 'light', 'colors', 'density', 'text size'], () => import('../theme/AppearancePage')),
  page('account', 'provider-accounts', 'Accounts for runs', Plug, ['provider accounts', 'sign in', 'sign-in', 'harness', 'runs', 'api key'], () => import('./account/ProviderAccountsPage'), { visible: runsSomewhere }),
  page('account', 'usage', 'Usage and alerts', 'activity', ['limits', 'rate limits', 'windows', 'resets', 'capacity', 'quota', 'banked reset', 'alerts'], () => import('./account/UsagePage'), { visible: runsSomewhere }),
  page('account', 'notifications', 'Notifications', Bell, ['push', 'phone', 'desktop', 'quiet hours', 'mute', 'email', 'digest', 'do not disturb'], () => import('./account/NotificationsPage'), { settings: ['notifications.'] }),
  page('account', 'connected-apps', 'Connected apps', 'link', ['mcp', 'mcp client', 'oauth', 'apps', 'connector'], () => import('./account/ConnectedAppsPage')),
  page('account', 'api-tokens', 'API tokens', 'key', ['token', 'bearer', 'mcp', 'credential', 'scripts'], () => import('./account/ApiTokensPage')),
  page('account', 'webhooks', 'Webhooks', Webhook, ['webhook', 'events', 'http', 'callback', 'integration', 'signed'], () => import('./account/WebhooksPage')),
  page('account', 'api-reference', 'API reference', BookOpen, ['rest', 'openapi', 'api', 'scripts', 'curl', 'docs'], () => import('./account/ApiReferencePage')),
  // Workspace
  page('workspace', 'general', 'General', 'settings', ['rename', 'workspace name', 'leave', 'transfer ownership', 'owner'], () => import('./workspace/GeneralPage')),
  page('workspace', 'members', 'Members', 'users', ['people', 'agents', 'roles', 'admin', 'invite', 'invitations', 'remove'], () => import('./workspace/MembersPage')),
  page('workspace', 'roles', 'Roles', UserCog, ['roles', 'permissions', 'access', 'custom role', 'contractor', 'read only', 'family', 'agent limit', 'what agents can do', 'default role'], () => import('./workspace/RolesPage')),
  page('workspace', 'keys', 'Keys', 'lock', ['secrets', 'api keys', 'credentials'], () => import('./workspace/KeysPage'), { visible: manager }),
  page('workspace', 'email', 'Email', Mail, ['smtp', 'mail', 'email', 'password reset', 'test email', 'outgoing mail'], () => import('./workspace/EmailPage'), { visible: owner }),
  page('workspace', 'runs', 'Runs and models', Cpu, ['harness', 'model', 'models', 'reasoning', 'speed', 'attempts', 'environment', 'environments', 'defaults', 'run defaults', 'model list', 'hide model', 'provider', 'suggestions', 'summaries', 'api key', 'budget', 'usage', 'spend', 'launch prompt', 'template', 'ai', 'anthropic', 'openai', 'gemini'], () => import('./workspace/RunsPage'), { settings: ['run.', 'ai.', 'launch.'] }),
  page('workspace', 'harnesses', 'Harnesses', 'terminal', ['custom harness', 'adapter', 'manifest', 'runner', 'command', 'agents'], () => import('./workspace/HarnessesPage')),
  page('workspace', 'project-templates', 'Project templates', LayoutTemplate, ['templates', 'new project', 'setup', 'starter'], () => import('./workspace/ProjectTemplatesPage')),
  page('workspace', 'layout', 'Layout', PanelsTopLeft, ['layout', 'presets', 'hide', 'show', 'simplify', 'personal', 'words', 'rename', 'sidebar', 'my view', 'hidden parts', 'default layout'], () => import('./workspace/LayoutPage')),
  page('workspace', 'modules', 'Modules', Blocks, ['modules', 'features', 'turn off', 'switch off', 'presets', 'projects', 'pages only', 'dms only', 'chat with my agents'], () => import('./workspace/ModulesPage')),
  page('workspace', 'github', 'GitHub', 'pull-request', ['installation', 'app', 'repositories'], () => import('./workspace/GitHubPage'), { visible: manager }),
  page('workspace', 'audit-log', 'Audit log', ScrollText, ['history', 'activity', 'who changed', 'events'], () => import('./workspace/AuditLogPage'), { visible: manager }),
  page('workspace', 'server', 'Server', Server, ['version', 'commit', 'backup', 'backups', 'health', 'preflight', 'disk', 'migrations', 'self-hosting'], () => import('./workspace/ServerPage'), { visible: serverOperator }),
  // Project
  page('project', 'general', 'General', 'settings', ['about', 'archive', 'key', 'name'], () => import('./project/GeneralPage')),
  page('project', 'layout', 'Layout', PanelsTopLeft, ['layout', 'presets', 'hide', 'show', 'simplify', 'roles', 'people', 'words', 'rename', 'sidebar', 'bug reporting', 'personal'], () => import('./project/LayoutPage')),
  page('project', 'members', 'Members', 'users', ['team', 'people', 'roles', 'invite'], () => import('./project/MembersPage'), {
    visible: (context) => manageProject(context) && Boolean(context.workspace),
  }),
  page('project', 'repositories', 'Repositories', 'repo', ['github', 'connect', 'mirror', 'code'], () => import('./project/RepositoriesPage'), { part: 'area.code' }),
  page('project', 'runs', 'Runs and models', Cpu, ['harness', 'model', 'models', 'reasoning', 'speed', 'attempts', 'environment', 'environments', 'repository', 'defaults', 'run defaults', 'provider', 'suggestions', 'summaries', 'dependencies', 'automatic', 'launch prompt', 'template', 'ai'], () => import('./project/RunsPage'), { settings: ['run.', 'ai.', 'launch.'], part: 'area.runs' }),
  page('project', 'agents', 'Agents', 'bot', ['external agents', 'connections', 'credentials', 'profiles', 'reviewer', 'planner', 'fixer', 'instructions'], () => import('./project/AgentsPage'), { visible: manageProject, part: 'area.agents' }),
  page('project', 'automations', 'Automation', Zap, ['rules', 'triggers', 'when', 'schedule', 'launch', 'checks failed', 'workflow automation', 'pull requests', 'review', 'github'], () => import('./project/AutomationRulesPage'), { visible: projectMember, part: 'area.automations' }),
  page('project', 'bug-reports', 'Bug reports', Bug, ['bug reports', 'crash reports', 'errors', 'intake', 'inbound', 'report link', 'feedback', 'other apps', 'webhook'], () => import('./project/BugReportsPage'), { visible: projectMember, part: 'area.bug_reports' }),
  page('project', 'workflow', 'Workflow', 'board', ['statuses', 'types', 'estimates', 'transitions', 'board', 'columns'], () => import('./project/WorkflowPage'), { visible: projectMember }),
  page('project', 'templates', 'Templates', 'bookmark', ['task templates', 'recurring', 'repeat', 'schedule', 'project template'], () => import('./project/TemplatesPage'), { visible: projectMember }),
  page('project', 'import-export', 'Import and export', ArrowDownUp, ['import', 'export', 'csv', 'markdown', 'github issues', 'linear', 'jira', 'json', 'download'], () => import('./project/ImportExportPage')),
  page('project', 'run-requests', 'Run requests', 'circle-check', ['run requests', 'sponsored', 'approve', 'provider account'], () => import('./project/CodexRunsPage'), { visible: projectMember, part: 'area.runs' }),
]

export const GROUP_LABELS: Record<SettingsGroup, string> = {
  account: 'Account',
  workspace: 'Workspace',
  project: 'Project',
}

export function pagesIn(group: SettingsGroup, context: SettingsVisibility) {
  return SETTINGS_PAGES.filter(
    (entry) =>
      entry.group === group &&
      (entry.visible?.(context) ?? true) &&
      (!entry.part || (context.experience?.shows(entry.part) ?? true)) &&
      !pageModuleOff(context, group, entry.id),
  )
}

export function findPage(group: SettingsGroup, id: string) {
  return SETTINGS_PAGES.find((entry) => entry.group === group && entry.id === id) ?? null
}

/** AWD-75: the module a settings page belongs to when it's off for the person, else null; search leaves the page out and opening it says the module is turned off. */
export function pageModuleOff(context: SettingsVisibility, group: SettingsGroup, id: string) {
  const module = ownerOf(settingsClaims, `${group}/${id}`)
  return module && !((context.modules ?? context.experience)?.moduleOn(module) ?? true) ? module : null
}
