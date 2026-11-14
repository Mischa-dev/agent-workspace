import { apiUrl, origins, readCsrfToken, requestJson } from '../api/client'
import { ApiError } from '../api/errors'
import { normalizeFile, normalizeFiles, type FileAttachment } from './model'

export {
  fileContentUrl,
  fileThumbnailUrl,
  formatBytes,
  isInlineImage,
  linkPreviewImageUrl,
  normalizeFile,
  normalizeFiles,
  normalizeLinkPreview,
  normalizeLinkPreviews,
  type FileAttachment,
  type LinkPreview,
} from './model'

function record(value: unknown): Record<string, unknown> | null {
  return value && typeof value === 'object' && !Array.isArray(value) ? (value as Record<string, unknown>) : null
}

/** Where an upload goes: a conversation (sent with a message) or a task (itself, or a comment on it). */
export type UploadTarget =
  | { kind: 'conversation'; conversationId: string }
  | { kind: 'task'; projectId: string; workItemId: string; forComment?: boolean }

export type Uploader = (file: File, onProgress: (fraction: number) => void, signal: AbortSignal) => Promise<FileAttachment>

function uploadPath(target: UploadTarget, name: string) {
  const query = `name=${encodeURIComponent(name)}`
  if (target.kind === 'conversation') {
    return `/api/v1/conversations/${encodeURIComponent(target.conversationId)}/files?${query}`
  }
  return `/api/v1/projects/${encodeURIComponent(target.projectId)}/work-items/${encodeURIComponent(target.workItemId)}/files?${query}${
    target.forComment ? '&for=comment' : ''
  }`
}

/** A pasted screenshot arrives as "image.png"; give it a name worth keeping. */
export function uploadName(file: File) {
  const name = file.name.trim()
  if (name && name !== 'image.png' && name !== 'image.jpeg') return name
  const extension = file.type === 'image/jpeg' ? 'jpg' : (file.type.split('/')[1] ?? 'png')
  const stamp = new Date().toISOString().slice(0, 19).replace('T', ' ').replace(/:/g, '.')
  return `Screenshot ${stamp}.${extension}`
}

/**
 * Sends the file's bytes as the request body, so a large file is never
 * base64-encoded. Uses XMLHttpRequest for upload progress, which fetch can't
 * report yet.
 */
export function uploadFile(target: UploadTarget): Uploader {
  return async (file, onProgress, signal) => {
    // On a static host the API is on another origin, and its request token
    // is read from there (H1.41), as requestJson does.
    const csrf = await (origins.csrf ?? readCsrfToken)()
    return new Promise<FileAttachment>((resolve, reject) => {
      const request = new XMLHttpRequest()
      request.open('POST', apiUrl(uploadPath(target, uploadName(file))))
      request.withCredentials = true
      request.setRequestHeader('accept', 'application/json')
      request.setRequestHeader('content-type', file.type || 'application/octet-stream')
      if (csrf) request.setRequestHeader('x-csrf-token', csrf)
      request.upload.onprogress = (event) => {
        if (event.lengthComputable && event.total > 0) onProgress(event.loaded / event.total)
      }
      request.onload = () => {
        let body: unknown
        try {
          body = JSON.parse(request.responseText)
        } catch {
          body = null
        }
        if (request.status >= 200 && request.status < 300) {
          const uploaded = normalizeFile(body)
          if (uploaded) resolve(uploaded)
          else reject(new ApiError({ status: request.status, code: 'invalid_response', message: 'The server returned an invalid response.' }))
          return
        }
        const error = record(record(body)?.error)
        reject(
          new ApiError({
            status: request.status,
            code: typeof error?.code === 'string' ? error.code : 'request_failed',
            message:
              request.status === 413
                ? `${file.name || 'That file'} is too large for this workspace.`
                : typeof error?.message === 'string'
                  ? error.message
                  : `Upload failed (${request.status}).`,
            requestId: request.getResponseHeader('x-request-id'),
          }),
        )
      }
      request.onerror = () => reject(new TypeError('Failed to fetch'))
      request.onabort = () => reject(new DOMException('The upload was canceled.', 'AbortError'))
      if (signal.aborted) {
        reject(new DOMException('The upload was canceled.', 'AbortError'))
        return
      }
      signal.addEventListener('abort', () => request.abort(), { once: true })
      request.send(file)
    })
  }
}

export async function removeFile(id: string) {
  await requestJson<unknown>(`/api/v1/files/${encodeURIComponent(id)}`, { method: 'DELETE' })
}

export async function listTaskFiles(projectId: string, workItemId: string): Promise<FileAttachment[]> {
  const response = await requestJson<unknown>(
    `/api/v1/projects/${encodeURIComponent(projectId)}/work-items/${encodeURIComponent(workItemId)}/files`,
  )
  return normalizeFiles(record(response)?.files)
}

export async function hideLinkPreview(input: { conversationId: string; messageId: string; previewId: string }) {
  await requestJson<unknown>(
    `/api/v1/conversations/${encodeURIComponent(input.conversationId)}/messages/${encodeURIComponent(input.messageId)}/link-previews/${encodeURIComponent(input.previewId)}`,
    { method: 'DELETE' },
  )
}
