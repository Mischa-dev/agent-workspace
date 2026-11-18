import { requestJson } from '../api/client'
import type { SessionResponse } from '../auth/session'

// First run in the browser (H1.14). These calls come before any account
// exists, so they can't be operations; they are the setup page's three
// hand-written calls, like sign-in's.

/** One shape a new workspace can start in (a built-in workspace template). */
export type SetupShape = {
  id: string
  name: string
  description: string
  whatItDoes: string[]
}

export type SetupStatus = {
  /** Nobody has an account yet: the setup page shows. */
  needed: boolean
  /** You made the first account to import a workspace and haven't sent it yet. */
  awaitingImport: boolean
  shapes: SetupShape[]
  maxArchiveBytes: number
}

export type SetupRequest = {
  code: string
  person: { displayName: string; loginName: string; password: string }
  /** Left out to import a workspace next. */
  workspace?: { name: string; shape?: string }
}

export const setupKeys = { status: ['setup', 'status'] as const }

export const setupStatus = () => requestJson<SetupStatus>('/api/v1/setup')

export const setUp = (request: SetupRequest) =>
  requestJson<SessionResponse>('/api/v1/setup', { method: 'POST', json: request })

/** The export archive, as the request's body; the import runs on the server. */
export const uploadArchive = (archive: File) =>
  requestJson<{ transferId: string; workspaceId: string }>('/api/v1/setup/import', {
    method: 'POST',
    body: archive,
    headers: { 'content-type': 'application/zip' },
  })
