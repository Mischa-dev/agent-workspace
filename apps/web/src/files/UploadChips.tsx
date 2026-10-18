import { Icon, IconButton } from '../ui'
import { fileThumbnailUrl, formatBytes, isInlineImage } from './api'
import type { PendingUpload } from './useUploads'
import './files.css'
import { FileGlyph } from './icons'

/** The files waiting in a composer: a thumbnail or icon, progress while uploading, and a way to take each out. */
export function UploadChips({ items, onRemove }: { items: PendingUpload[]; onRemove: (localId: string) => void }) {
  if (items.length === 0) return null
  return (
    <ul aria-label="Attached files" className="aw-uploads">
      {items.map((item) => {
        const picture = item.previewUrl ?? (item.file && isInlineImage(item.file) ? fileThumbnailUrl(item.file.id) : null)
        const state = item.error ? 'is-failed' : item.file ? 'is-done' : 'is-uploading'
        return (
          <li className={`aw-upload ${state}`} key={item.localId} title={item.error ?? item.name}>
            {picture ? (
              <img alt="" className="aw-upload__thumb" src={picture} />
            ) : (
              <span aria-hidden="true" className="aw-upload__icon">
                <Icon name={item.error ? 'circle-alert' : FileGlyph} size={18} />
              </span>
            )}
            <span className="aw-upload__text">
              <span className="aw-upload__name">{item.name}</span>
              <span className="aw-upload__meta" role={item.error ? 'alert' : undefined}>
                {item.error ?? (item.file ? formatBytes(item.size) : `Uploading ${Math.round(item.progress * 100)}%`)}
              </span>
            </span>
            {!item.file && !item.error ? (
              <span aria-hidden="true" className="aw-upload__progress" style={{ transform: `scaleX(${item.progress})` }} />
            ) : null}
            <IconButton className="aw-upload__remove" icon="x" label={`Remove ${item.name}`} onClick={() => onRemove(item.localId)} size="sm" tooltip={false} />
          </li>
        )
      })}
    </ul>
  )
}
