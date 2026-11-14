// Workspace › Email and Workspace › Server: both are for owners only.
import { requestJson } from '../../api/client'

export type MailSecurity = 'starttls' | 'tls' | 'none'

export type SmtpSettings = {
  host: string
  port: number
  security: MailSecurity
  username: string | null
}

export type MailSettings = {
  configured: boolean
  enabled: boolean
  provider: 'smtp' | null
  smtp: SmtpSettings | null
  fromAddress: string | null
  fromName: string | null
  passwordSet: boolean
  lastTest: { at: string; ok: boolean; error: string | null } | null
  updatedAt: string | null
  /** Your confirmed address, where a test goes by default. */
  yourAddress: string | null
}

export type SaveMailSettings = {
  provider: 'smtp'
  smtp: SmtpSettings
  fromAddress: string
  fromName: string | null
  enabled: boolean
  password?: string
  clearPassword?: boolean
}

export type TestEmailResult = { ok: boolean; sentTo: string; error: string | null }

export type CheckLevel = 'ok' | 'warn' | 'fail'

export type BackupState = 'never' | 'ok' | 'failed' | 'overdue' | 'unreadable'

export type ServerStatus = {
  version: string
  commit: string
  environment: string
  publicOrigin: string
  checkedAt: string
  checks: { level: CheckLevel; text: string }[]
  backup: {
    state: BackupState
    last: { startedAt: string; finishedAt: string; ok: boolean; steps: Record<string, string> } | null
  }
  mail: { configured: boolean; enabled: boolean; lastTestAt: string | null; lastTestOk: boolean | null }
  jobs: { waiting: number; running: number; failedLastDay: number; oldestWaitingSeconds: number }
  /** The event log: events waiting for a position, and events a consumer parked. Older servers leave it out. */
  eventLog?: { waiting: number; oldestWaitingSeconds: number; parked: number }
}

export const mailKeys = {
  settings: (organizationId: string) => ['workspace', organizationId, 'mail'] as const,
  server: (organizationId: string) => ['workspace', organizationId, 'server'] as const,
}

const mailPath = (organizationId: string) => `/api/v1/organizations/${encodeURIComponent(organizationId)}/mail`

export const getMailSettings = (organizationId: string) => requestJson<MailSettings>(mailPath(organizationId))

export const saveMailSettings = (organizationId: string, settings: SaveMailSettings) =>
  requestJson<MailSettings>(mailPath(organizationId), { method: 'PUT', json: settings })

export const removeMailSettings = (organizationId: string) => requestJson<void>(mailPath(organizationId), { method: 'DELETE' })

export const sendTestEmail = (organizationId: string, to: string | null) =>
  requestJson<TestEmailResult>(`${mailPath(organizationId)}/test`, { method: 'POST', json: to ? { to } : {} })

export const getServerStatus = (organizationId: string) =>
  requestJson<ServerStatus>(`/api/v1/organizations/${encodeURIComponent(organizationId)}/server`)
