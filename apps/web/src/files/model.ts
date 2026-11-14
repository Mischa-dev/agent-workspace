// Files and link previews as the server sends them (W20): types, checks and
// URLs, small enough for the initial load. Uploading lives in ./api.

import { apiUrl } from '../api/client'

/** A file on a message, task or comment. Its URLs are built from the id and checked against the session. */
export type FileAttachment = {
  id: string
  name: string
  contentType: string
  size: number
  /** Set for images shown inline. */
  width: number | null
  height: number | null
  /** The task comment it was posted with. */
  commentId: string | null
  uploadedBy: string
  createdAt: string
}

/** A web page's title, description and image, under a message that links to it. */
export type LinkPreview = {
  id: string
  url: string
  title: string
  description: string | null
  siteName: string | null
  imageWidth: number | null
  imageHeight: number | null
}

type WireRecord = Record<string, unknown>

function record(value: unknown): WireRecord | null {
  return value && typeof value === 'object' && !Array.isArray(value) ? (value as WireRecord) : null
}

function positive(value: unknown) {
  return typeof value === 'number' && Number.isFinite(value) && value > 0 ? Math.round(value) : null
}

const ID = /^[0-9a-f-]{36}$/i

export function normalizeFile(value: unknown): FileAttachment | null {
  const file = record(value)
  if (!file || typeof file.id !== 'string' || !ID.test(file.id) || typeof file.name !== 'string') return null
  return {
    id: file.id,
    name: file.name.slice(0, 255),
    contentType: typeof file.contentType === 'string' ? file.contentType : 'application/octet-stream',
    size: typeof file.size === 'number' && file.size >= 0 ? file.size : 0,
    width: positive(file.width),
    height: positive(file.height),
    commentId: typeof file.commentId === 'string' ? file.commentId : null,
    uploadedBy: typeof file.uploadedBy === 'string' ? file.uploadedBy : '',
    createdAt: typeof file.createdAt === 'string' ? file.createdAt : '',
  }
}

export function normalizeFiles(value: unknown): FileAttachment[] {
  return Array.isArray(value) ? value.map(normalizeFile).filter((file): file is FileAttachment => file !== null) : []
}

export function normalizeLinkPreview(value: unknown): LinkPreview | null {
  const preview = record(value)
  if (!preview || typeof preview.id !== 'string' || !ID.test(preview.id)) return null
  if (typeof preview.url !== 'string' || !/^https?:\/\//i.test(preview.url)) return null
  if (typeof preview.title !== 'string' || !preview.title.trim()) return null
  return {
    id: preview.id,
    url: preview.url,
    title: preview.title.slice(0, 300),
    description: typeof preview.description === 'string' ? preview.description.slice(0, 600) : null,
    siteName: typeof preview.siteName === 'string' ? preview.siteName.slice(0, 100) : null,
    imageWidth: positive(preview.imageWidth),
    imageHeight: positive(preview.imageHeight),
  }
}

export function normalizeLinkPreviews(value: unknown): LinkPreview[] {
  return Array.isArray(value)
    ? value.map(normalizeLinkPreview).filter((preview): preview is LinkPreview => preview !== null)
    : []
}

export const fileContentUrl = (id: string, download = false) =>
  apiUrl(`/api/v1/files/${encodeURIComponent(id)}/content${download ? '?download=1' : ''}`)
export const fileThumbnailUrl = (id: string) => apiUrl(`/api/v1/files/${encodeURIComponent(id)}/thumbnail`)
export const linkPreviewImageUrl = (id: string) => `/api/v1/link-previews/${encodeURIComponent(id)}/image`

/** Shown inline as a picture (the server sets width and height only for those). */
export const isInlineImage = (file: FileAttachment) => file.width !== null && file.height !== null

export function formatBytes(bytes: number) {
  if (bytes < 1024) return `${bytes} B`
  const units = ['KB', 'MB', 'GB']
  let value = bytes / 1024
  let unit = 0
  while (value >= 1024 && unit < units.length - 1) {
    value /= 1024
    unit += 1
  }
  return `${value < 10 ? value.toFixed(1) : Math.round(value)} ${units[unit]}`
}
