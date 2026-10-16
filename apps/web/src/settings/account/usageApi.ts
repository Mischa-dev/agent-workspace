import { requestJson } from '../../api/client'

// Usage windows, capacity alerts and banked resets (U1, U3, U6), and where
// notifications reach you (N3, N4). Accounts belong to one person: everything
// here is about the signed-in person's own accounts and devices.

export type WindowSource = 'reported' | 'schedule'

export type UsageWindow = {
  key: string
  /** Such as 5-hour, Weekly, or a schedule's name. */
  label: string
  source: WindowSource
  scheduleId: string | null
  durationMinutes: number | null
  /** Null when nobody knows (a schedule you set). */
  usedPercent: number | null
  unusedPercent: number | null
  resetsAt: string | null
  limitReached: boolean
  readAt: string | null
  rolledOver: boolean
}

export type ResetCredit = { id: string; title: string | null; expiresAt: string | null; grantedAt: string | null }

export type UsageSchedule = {
  id: string
  label: string
  cadence: 'daily' | 'weekly' | 'monthly'
  weekday: number | null
  monthDay: number | null
  minuteOfDay: number
  timeZone: string
}

export type AlertRuleName = 'reset_soon' | 'high_usage' | 'limit_reached' | 'reset_credits'

export type AlertRule = {
  rule: AlertRuleName
  description: string
  enabled: boolean
  hours: number | null
  percent: number | null
  defaultHours: number | null
  defaultPercent: number | null
}

export type AccountUsage = {
  accountId: string
  organizationId: string
  name: string
  harness: string
  harnessName: string
  connected: boolean
  plan: string | null
  polledAt: string | null
  pollFailedAt: string | null
  pollFailure: string | null
  windows: UsageWindow[]
  resetCredits: { available: number; credits: ResetCredit[] } | null
  schedules: UsageSchedule[]
  rules: AlertRule[]
}

export type ReadyTask = {
  id: string
  projectId: string
  projectName: string
  key: string
  title: string
  priority: string
  unblocks: number
  href: string
}

export type ReadyTasks = { tasks: ReadyTask[]; projectCount: number }

export const usageKeys = {
  root: ['usage'] as const,
  list: ['usage', 'accounts'] as const,
  ready: (accountId: string) => ['usage', 'ready', accountId] as const,
}

const account = (accountId: string) => `/api/v1/usage/accounts/${encodeURIComponent(accountId)}`

export const listUsage = () => requestJson<{ accounts: AccountUsage[] }>('/api/v1/usage')

export const refreshUsage = (accountId: string) =>
  requestJson<AccountUsage>(`${account(accountId)}/refresh`, { method: 'POST', json: {} })

export const readyTasks = (accountId: string) => requestJson<ReadyTasks>(`${account(accountId)}/ready-tasks`)

export type NewSchedule = Omit<UsageSchedule, 'id'>

export const addSchedule = (accountId: string, schedule: NewSchedule) =>
  requestJson<AccountUsage>(`${account(accountId)}/schedules`, { method: 'POST', json: schedule })

export const removeSchedule = (accountId: string, scheduleId: string) =>
  requestJson<AccountUsage>(`${account(accountId)}/schedules/${encodeURIComponent(scheduleId)}`, { method: 'DELETE' })

export const setRule = (
  accountId: string,
  rule: AlertRuleName,
  change: { enabled: boolean; hours: number | null; percent: number | null },
) => requestJson<AccountUsage>(`${account(accountId)}/rules/${rule}`, { method: 'PUT', json: change })

export type SpendOutcome = 'reset' | 'nothing_to_reset' | 'no_credit' | 'already_redeemed'

/** Spending can't be undone. The key makes a retried click spend at most one. */
export const spendResetCredit = (accountId: string, creditId: string | null, idempotencyKey: string) =>
  requestJson<{ outcome: SpendOutcome; account: AccountUsage }>(`${account(accountId)}/reset-credits/spend`, {
    method: 'POST',
    json: { creditId, confirm: true },
    idempotencyKey,
  })

// ---------------------------------------------------------------------------
// Notification settings

export type NotificationGroup = 'mentions' | 'approvals' | 'runs' | 'tasks' | 'pull_requests' | 'usage' | 'other'
export type NotificationChannel = 'in_app' | 'push' | 'email'

export type GroupSetting = { group: NotificationGroup; label: string; inApp: boolean; push: boolean; email: boolean }
export type ProjectMute = { projectId: string; projectName: string; group: NotificationGroup | 'all' }
export type QuietHours = { enabled: boolean; start: number; end: number; timeZone: string | null }

export type NotificationSettings = {
  organizationId: string
  groups: GroupSetting[]
  mutes: ProjectMute[]
  quietHours: QuietHours
  digest: 'off' | 'daily' | 'weekly'
  emailAvailable: boolean
}

export const notificationKeys = {
  settings: ['notification-settings'] as const,
  devices: ['push', 'devices'] as const,
}

export const getNotificationSettings = () => requestJson<NotificationSettings>('/api/v1/notification-settings')

export const setChannel = (organizationId: string, group: NotificationGroup, channel: NotificationChannel, enabled: boolean) =>
  requestJson<NotificationSettings>('/api/v1/notification-settings/channels', {
    method: 'PUT',
    json: { organizationId, group, channel, enabled },
  })

export const setMute = (projectId: string, group: NotificationGroup | 'all', muted: boolean) =>
  requestJson<NotificationSettings>('/api/v1/notification-settings/mutes', {
    method: 'PUT',
    json: { projectId, group, muted },
  })

/** Quiet hours, time zone and digest are personal settings in the settings registry. */
export const setPersonalSetting = (organizationId: string, key: string, value: unknown) =>
  requestJson<unknown>(`/api/v1/settings/values/${encodeURIComponent(key)}`, {
    method: 'PUT',
    json: { scope: 'user', organizationId, value },
  })

// ---------------------------------------------------------------------------
// Push devices

export type PushDevice = {
  id: string
  label: string
  serviceHost: string
  endpointHash: string
  createdAt: string
  lastSentAt: string | null
  lastFailureAt: string | null
  failureCount: number
}

export const getPushConfig = () => requestJson<{ organizationId: string; publicKey: string }>('/api/v1/push/config')

export const listDevices = () => requestJson<{ devices: PushDevice[] }>('/api/v1/push/subscriptions')

export const addDevice = (body: {
  endpoint: string
  keys: { p256dh: string; auth: string }
  organizationId: string
  label: string
}) => requestJson<PushDevice>('/api/v1/push/subscriptions', { method: 'POST', json: body })

export const removeDevice = (id: string) =>
  requestJson<void>(`/api/v1/push/subscriptions/${encodeURIComponent(id)}`, { method: 'DELETE' })

export const testDevice = (id: string) =>
  requestJson<{ result: 'sent' | 'gone' | 'retry' | 'failed' }>(`/api/v1/push/subscriptions/${encodeURIComponent(id)}/test`, {
    method: 'POST',
    json: {},
  })
