import { here } from '../app/href'
import { apiUrl, requestJson } from '../api/client'
import { callOp } from '../api/ops'
import { isDarkScheme, type ThemePreset } from '../theme/slots'

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
  /** A template: offered in "New whiteboard" as something to start from. */
  isTemplate: boolean
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
  /** The role only suggests here: drawing is sent as a proposal (C1.11). */
  canSuggest?: boolean
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

export { whiteboardKeys } from './keys'

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

/** Makes a whiteboard through `whiteboard_create`, then reads it back whole for the page it opens. */
export async function createWhiteboard(input: {
  title: string
  organizationId?: string
  projectId?: string
  link?: { targetKind: LinkTargetKind; targetId: string }
  /** Start from this template: a template board's id or a built-in's name. */
  fromTemplateId?: string
}) {
  const { link, organizationId, ...rest } = input
  const created = await callOp('whiteboard_create', { ...rest, workspaceId: organizationId, linkTo: link })
  return getWhiteboard(created.whiteboard.id)
}

export function getWhiteboard(id: string) {
  return requestJson<WhiteboardDetail>(board(id))
}

/** Renames, trashes, restores or makes a template of a whiteboard (`whiteboard_update`), and reads it back. */
export async function updateWhiteboard(
  id: string,
  change: { title?: string; projectId?: string; deleted?: boolean; isTemplate?: boolean; expectedVersion?: number },
) {
  await callOp('whiteboard_update', { whiteboardId: id, ...change })
  return getWhiteboard(id)
}

/** Deletes a whiteboard in the trash for good, with its drawing, images and history. */
export function deleteWhiteboardForever(id: string) {
  return requestJson<void>(board(id), { method: 'DELETE' })
}

/** Deletes for good every whiteboard in the trash you can change. */
export function emptyWhiteboardTrash() {
  return requestJson<{ deleted: number }>('/api/v1/whiteboard-trash/empty', { method: 'POST', json: {} })
}

export async function duplicateWhiteboard(id: string, title?: string) {
  const copy = await callOp('whiteboard_duplicate', { whiteboardId: id, title })
  return getWhiteboard(copy.whiteboard.id)
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


/** Shows a whiteboard on a task, project or page (`whiteboard_link`); linking twice changes nothing. */
export function linkWhiteboard(id: string, targetKind: LinkTargetKind, targetId: string) {
  return callOp('whiteboard_link', { whiteboardId: id, targetKind, targetId })
}

export function unlinkWhiteboard(id: string, targetKind: string, targetId: string) {
  return callOp('whiteboard_link', { whiteboardId: id, targetKind, targetId, remove: true })
}

export function linkedWhiteboards(targetKind: LinkTargetKind, targetId: string) {
  const query = new URLSearchParams({ targetKind, targetId })
  return requestJson<{ whiteboards: Whiteboard[] }>(`/api/v1/whiteboard-links?${query.toString()}`)
}

export function previewUrl(id: string, version: number, dark: boolean) {
  return apiUrl(`${board(id)}/preview.svg?dark=${dark ? 'true' : 'false'}&v=${version}`)
}

/** Whether whiteboards draw dark: on every dark built-in theme. */
export function isDarkPreset(preset: ThemePreset) {
  return isDarkScheme(preset)
}

export function whiteboardPath(id: string) {
  return here(`/whiteboards/${encodeURIComponent(id)}`)
}
