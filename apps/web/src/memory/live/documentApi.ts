import { requestJson } from '../../api/client'

/** A page's live document (AWD-53): its whole Yjs state, base64. */
export type PageDocument = {
  pageId: string
  /** The document's lineage; a rebuilt document gets a new one. */
  generation: string
  version: number
  state: string
  /** Whether you may change it (an archived page needs bringing back first). */
  canEdit: boolean
}

/**
 * `synced`: `update` is what this copy lacked. `resync`: the server lacks
 * something the change built on; send what `stateVector` (the server's)
 * lacks. `replaced`: the page's document was rebuilt; load it again.
 */
export type PageSyncReply = {
  status: 'synced' | 'resync' | 'replaced'
  generation: string
  version: number
  update?: string
  stateVector?: string
}

const documentPath = (pageId: string) => `/api/v1/pages/${encodeURIComponent(pageId)}/document`

export function getPageDocument(pageId: string) {
  return requestJson<PageDocument>(documentPath(pageId))
}

/** What this copy lacks, given its state vector. A read: viewers may sync. */
export function syncPageDocument(pageId: string, body: { generation: string; stateVector: string }) {
  return requestJson<PageSyncReply>(`${documentPath(pageId)}/sync`, { method: 'POST', json: body })
}

/** This copy's changes the server may lack, with what this copy lacks in the reply. */
export function sendPageUpdate(pageId: string, body: { generation: string; update: string; stateVector: string }) {
  return requestJson<PageSyncReply>(`${documentPath(pageId)}/updates`, { method: 'POST', json: body })
}

export function toBase64(bytes: Uint8Array) {
  let binary = ''
  // In slices, so a large document doesn't overflow the call stack.
  for (let start = 0; start < bytes.length; start += 0x8000) {
    binary += String.fromCharCode(...bytes.subarray(start, start + 0x8000))
  }
  return btoa(binary)
}

export function fromBase64(text: string) {
  const binary = atob(text)
  const bytes = new Uint8Array(binary.length)
  for (let index = 0; index < binary.length; index += 1) bytes[index] = binary.charCodeAt(index)
  return bytes
}
