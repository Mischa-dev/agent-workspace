import { useEffect, useRef, useState } from 'react'

import { removeFile, uploadName, type FileAttachment, type Uploader } from './api'

/** Most files on one message or comment (the server's limit too). */
export const MAX_ATTACHMENTS = 10

export type PendingUpload = {
  localId: string
  name: string
  size: number
  /** A local preview for pictures, while they upload. */
  previewUrl: string | null
  progress: number
  file: FileAttachment | null
  error: string | null
}

/**
 * Files picked, pasted or dropped into a composer: each uploads at once (so
 * sending is quick), shows its progress, and can be taken out again, which
 * removes the upload from the server too.
 */
export function useUploads(upload: Uploader | undefined, initial: FileAttachment[] = []) {
  const [items, setItems] = useState<PendingUpload[]>(() =>
    initial.map((file) => ({ localId: file.id, name: file.name, size: file.size, previewUrl: null, progress: 1, file, error: null })),
  )
  const controllers = useRef(new Map<string, AbortController>())
  const itemsRef = useRef(items)
  useEffect(() => {
    itemsRef.current = items
  })

  // Uploads still running when the composer goes away are canceled; local previews are freed.
  useEffect(() => {
    const running = controllers.current
    return () => {
      for (const controller of running.values()) controller.abort()
      for (const item of itemsRef.current) if (item.previewUrl) URL.revokeObjectURL(item.previewUrl)
    }
  }, [])

  const update = (localId: string, change: Partial<PendingUpload>) =>
    setItems((current) => current.map((item) => (item.localId === localId ? { ...item, ...change } : item)))

  const add = (files: Iterable<File>): string | null => {
    if (!upload) return null
    const room = MAX_ATTACHMENTS - itemsRef.current.length
    const picked = [...files]
    if (picked.length === 0) return null
    const accepted = picked.slice(0, Math.max(0, room))
    const added = accepted.map((file) => {
      const localId = globalThis.crypto.randomUUID()
      const previewUrl = /^image\/(png|jpeg|gif|webp)$/.test(file.type) && typeof URL.createObjectURL === 'function' ? URL.createObjectURL(file) : null
      return { file, item: { localId, name: uploadName(file), size: file.size, previewUrl, progress: 0, file: null, error: null } }
    })
    setItems((current) => [...current, ...added.map(({ item }) => item)])
    for (const { file, item } of added) {
      const controller = new AbortController()
      controllers.current.set(item.localId, controller)
      upload(file, (progress) => update(item.localId, { progress }), controller.signal)
        .then((uploaded) => update(item.localId, { file: uploaded, progress: 1 }))
        .catch((reason: unknown) => {
          if (controller.signal.aborted) return
          update(item.localId, { error: reason instanceof Error ? reason.message : 'The file wasn’t uploaded.' })
        })
        .finally(() => controllers.current.delete(item.localId))
    }
    return accepted.length < picked.length ? `You can attach up to ${MAX_ATTACHMENTS} files at a time.` : null
  }

  const remove = (localId: string) => {
    const item = itemsRef.current.find((entry) => entry.localId === localId)
    controllers.current.get(localId)?.abort()
    if (item?.previewUrl) URL.revokeObjectURL(item.previewUrl)
    // An upload nobody sends is cleaned up after a day anyway; this is just sooner.
    if (item?.file) void removeFile(item.file.id).catch(() => undefined)
    setItems((current) => current.filter((entry) => entry.localId !== localId))
  }

  /** Forget the list without removing anything, once the files are sent. */
  const clear = () => {
    for (const item of itemsRef.current) if (item.previewUrl) URL.revokeObjectURL(item.previewUrl)
    setItems([])
  }

  /** Put files back, for a send that failed. */
  const restore = (files: FileAttachment[]) =>
    setItems((current) => [
      ...files
        .filter((file) => !current.some((item) => item.file?.id === file.id))
        .map((file) => ({ localId: file.id, name: file.name, size: file.size, previewUrl: null, progress: 1, file, error: null })),
      ...current,
    ])

  const ready = items.flatMap((item) => (item.file ? [item.file] : []))
  const uploading = items.some((item) => !item.file && !item.error)
  const failed = items.some((item) => item.error)
  return { items, add, remove, clear, restore, ready, uploading, failed }
}
