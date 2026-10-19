import { requestJson } from '../api/client'
import { THEME_PRESETS, type ThemePreset } from '../theme/slots'

export type Whiteboard = {
  id: string
  organizationId: string
  organizationName: string
  /** The project whose members see it; null for the whole workspace. */
  projectId: string | null
  projectName: string | null
  title: string
  sceneSeq: number
  version: number
  elementCount: number
  createdAt: string
  createdBy: string | null
  updatedAt: string
  sceneUpdatedAt: string
  updatedBy: string | null
  updatedByKind: string | null
  deleted: boolean
  linkCount: number
}

export type WhiteboardLink = {
  targetKind: 'project' | 'work_item' | 'page'
  targetId: string
  projectId: string
  projectName: string
  title: string
  key: string | null
  url: string
}

export type WhiteboardRevision = {
  revision: number
  sceneSeq: number
  elementCount: number
  author: string | null
  note: string | null
  createdAt: string
}

export type WhiteboardDetail = {
  whiteboard: Whiteboard
  links: WhiteboardLink[]
  canEdit: boolean
  revisions: WhiteboardRevision[]
}

export type Workspace = { id: string; name: string }

export type Editor = { actorId: string; name: string; kind: string; at: string }

/** Excalidraw elements as the server stores them. */
export type StoredElement = {
  id: string
  version: number
  versionNonce: number
  isDeleted?: boolean
  [field: string]: unknown
}

export type SceneView = {
  whiteboardId: string
  sceneSeq: number
  elements: StoredElement[]
  files: { id: string; mimeType: string }[]
  recentEditors: Editor[]
}

export type WriteOutcome = { sceneSeq: number; accepted: string[]; rejected: StoredElement[] }

export type StoredFile = { id: string; mimeType: string; dataURL: string; created: number }

export type LinkTargetKind = 'project' | 'task' | 'page'

export const whiteboardKeys = {
  all: ['whiteboards'] as const,
  list: (filters: Record<string, unknown>) => ['whiteboards', 'list', filters] as const,
  detail: (id: string) => ['whiteboards', 'detail', id] as const,
  linked: (kind: LinkTargetKind, targetId: string) => ['whiteboards', 'linked', kind, targetId] as const,
}

const base = '/api/v1/whiteboards'
const board = (id: string) => `${base}/${encodeURIComponent(id)}`

export function listWhiteboards(filters: { q?: string; deleted?: boolean; projectId?: string }) {
  const query = new URLSearchParams()
  if (filters.q?.trim()) query.set('q', filters.q.trim())
  if (filters.deleted) query.set('deleted', 'true')
  if (filters.projectId) query.set('projectId', filters.projectId)
  const suffix = query.size ? `?${query.toString()}` : ''
  return requestJson<{ whiteboards: Whiteboard[]; workspaces: Workspace[] }>(`${base}${suffix}`)
}

export function createWhiteboard(input: {
  title: string
  organizationId?: string
  projectId?: string
  link?: { targetKind: LinkTargetKind; targetId: string }
}) {
  return requestJson<WhiteboardDetail>(base, { method: 'POST', json: input })
}

export function getWhiteboard(id: string) {
  return requestJson<WhiteboardDetail>(board(id))
}

export function updateWhiteboard(
  id: string,
  change: { title?: string; projectId?: string | null; deleted?: boolean; expectedVersion?: number },
) {
  return requestJson<WhiteboardDetail>(board(id), { method: 'PATCH', json: change })
}

export function duplicateWhiteboard(id: string, title?: string) {
  return requestJson<WhiteboardDetail>(`${board(id)}/duplicate`, { method: 'POST', json: title ? { title } : {} })
}

export function getScene(id: string, since?: number) {
  const suffix = since === undefined ? '' : `?since=${since}`
  return requestJson<SceneView>(`${board(id)}/scene${suffix}`)
}

export function writeScene(id: string, elements: StoredElement[]) {
  return requestJson<WriteOutcome>(`${board(id)}/scene`, { method: 'POST', json: { elements } })
}

export function uploadFile(id: string, file: { id: string; mimeType: string; dataURL: string }) {
  return requestJson<{ id: string; stored: boolean }>(`${board(id)}/files`, { method: 'POST', json: file })
}

export function getFile(id: string, fileId: string) {
  return requestJson<StoredFile>(`${board(id)}/files/${encodeURIComponent(fileId)}`)
}

export function restoreRevision(id: string, revision: number) {
  return requestJson<WriteOutcome>(`${board(id)}/revisions/${revision}/restore`, { method: 'POST', json: {} })
}

export function linkWhiteboard(id: string, targetKind: LinkTargetKind, targetId: string) {
  return requestJson<WhiteboardDetail>(`${board(id)}/links`, { method: 'POST', json: { targetKind, targetId } })
}

export function unlinkWhiteboard(id: string, targetKind: string, targetId: string) {
  return requestJson<WhiteboardDetail>(
    `${board(id)}/links/${encodeURIComponent(targetKind)}/${encodeURIComponent(targetId)}`,
    { method: 'DELETE' },
  )
}

export function linkedWhiteboards(targetKind: LinkTargetKind, targetId: string) {
  const query = new URLSearchParams({ targetKind, targetId })
  return requestJson<{ whiteboards: Whiteboard[] }>(`/api/v1/whiteboard-links?${query.toString()}`)
}

export function previewUrl(id: string, version: number, dark: boolean) {
  return `${board(id)}/preview.svg?dark=${dark ? 'true' : 'false'}&v=${version}`
}

/** Whether whiteboards draw dark: Midnight and every classic dark theme. */
export function isDarkPreset(preset: ThemePreset) {
  return preset === 'midnight' || THEME_PRESETS.some((entry) => entry.id === preset && entry.group === 'dark')
}

export function whiteboardPath(id: string) {
  return `/whiteboards/${encodeURIComponent(id)}`
}
