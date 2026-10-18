import { uploadFile, type FileAttachment, type UploadTarget } from '../files/api'

const SHARE_CACHE = 'aw-share-v1'

/**
 * The files another app shared, which the service worker kept for this page
 * (see receiveShare in public/sw.js). Taking them deletes them there.
 */
export async function takeSharedFiles(count: number): Promise<File[]> {
  if (count <= 0 || typeof caches === 'undefined') return []
  try {
    const cache = await caches.open(SHARE_CACHE)
    const files: File[] = []
    for (let index = 0; index < Math.min(count, 10); index += 1) {
      const response = await cache.match(`/__shared/${index}`)
      if (!response) continue
      const blob = await response.blob()
      let name = `file-${index + 1}`
      try {
        name = decodeURIComponent(response.headers.get('x-file-name') ?? name)
      } catch {
        // Keep the plain name.
      }
      files.push(new File([blob], name, { type: response.headers.get('content-type') ?? blob.type }))
    }
    await caches.delete(SHARE_CACHE)
    return files
  } catch {
    return []
  }
}

/** Upload shared files to where they're going, in order. */
export async function uploadShared(files: File[], target: UploadTarget): Promise<FileAttachment[]> {
  const upload = uploadFile(target)
  const uploaded: FileAttachment[] = []
  for (const file of files) {
    uploaded.push(await upload(file, () => undefined, new AbortController().signal))
  }
  return uploaded
}
