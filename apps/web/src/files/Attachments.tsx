import { useEffect, useRef, useState } from 'react'
import { createPortal } from 'react-dom'

import { Icon, IconButton, type IconGlyph } from '../ui'
import {
  fileContentUrl,
  fileThumbnailUrl,
  formatBytes,
  hideLinkPreview,
  isInlineImage,
  linkPreviewImageUrl,
  type FileAttachment,
  type LinkPreview,
} from './api'
import './files.css'
import { Download, FileGlyph, FileText, ImageGlyph } from './icons'

function fileIcon(file: FileAttachment): IconGlyph {
  if (file.contentType.startsWith('image/')) return ImageGlyph
  if (file.contentType.startsWith('text/') || /json|yaml|toml|markdown|csv/.test(file.contentType)) return FileText
  return FileGlyph
}

/** A picture's box: its own shape, never more than 360 by 280 pixels. */
function thumbnailSize(file: FileAttachment, single: boolean) {
  const maxWidth = single ? 360 : 160
  const maxHeight = single ? 280 : 160
  const width = file.width ?? maxWidth
  const height = file.height ?? maxHeight
  const scale = Math.min(1, maxWidth / width, maxHeight / height)
  return { width: Math.max(48, Math.round(width * scale)), height: Math.max(48, Math.round(height * scale)) }
}

/** A message's or comment's files: pictures as thumbnails that open full size, anything else as a card to download. */
export function Attachments({ files, onRemove }: { files: FileAttachment[]; onRemove?: (file: FileAttachment) => void }) {
  const [open, setOpen] = useState<FileAttachment | null>(null)
  if (files.length === 0) return null
  const images = files.filter(isInlineImage)
  const others = files.filter((file) => !isInlineImage(file))
  return (
    <div className="aw-files">
      {images.length ? (
        <div className={images.length === 1 ? 'aw-files__images is-single' : 'aw-files__images'}>
          {images.map((file) => {
            const size = thumbnailSize(file, images.length === 1)
            return (
              <div className="aw-files__image-wrap" key={file.id}>
                <button
                  aria-label={`Open ${file.name}`}
                  className="aw-files__image"
                  onClick={() => setOpen(file)}
                  style={{ width: size.width, height: size.height }}
                  type="button"
                >
                  <img alt={file.name} decoding="async" height={size.height} loading="lazy" src={fileThumbnailUrl(file.id)} width={size.width} />
                </button>
                {onRemove ? (
                  <IconButton className="aw-files__remove" icon="trash" label={`Remove ${file.name}`} onClick={() => onRemove(file)} size="sm" />
                ) : null}
              </div>
            )
          })}
        </div>
      ) : null}
      {others.map((file) => (
        <div className="aw-file" key={file.id}>
          <span aria-hidden="true" className="aw-file__icon">
            <Icon name={fileIcon(file)} size={20} />
          </span>
          <a className="aw-file__main" download={file.name} href={fileContentUrl(file.id, true)} title={`Download ${file.name}`}>
            <span className="aw-file__name">{file.name}</span>
            <span className="aw-file__meta">{formatBytes(file.size)}</span>
          </a>
          <a aria-label={`Download ${file.name}`} className="aw-iconbtn aw-iconbtn--sm" download={file.name} href={fileContentUrl(file.id, true)}>
            <Icon name={Download} />
          </a>
          {onRemove ? <IconButton icon="trash" label={`Remove ${file.name}`} onClick={() => onRemove(file)} size="sm" /> : null}
        </div>
      ))}
      {open ? <ImageViewer file={open} images={images} onClose={() => setOpen(null)} onShow={setOpen} /> : null}
    </div>
  )
}

/** A picture full size over the page. Arrow keys move between a message's pictures; Escape or a tap outside closes it. */
function ImageViewer({
  file,
  images,
  onClose,
  onShow,
}: {
  file: FileAttachment
  images: FileAttachment[]
  onClose: () => void
  onShow: (file: FileAttachment) => void
}) {
  const closeButton = useRef<HTMLButtonElement>(null)
  const index = images.findIndex((image) => image.id === file.id)
  const step = (by: number) => {
    const next = images[(index + by + images.length) % images.length]
    if (next) onShow(next)
  }
  const stepRef = useRef(step)
  const closeRef = useRef(onClose)
  useEffect(() => {
    stepRef.current = step
    closeRef.current = onClose
  })
  useEffect(() => {
    const restore = document.activeElement as HTMLElement | null
    closeButton.current?.focus()
    const onKey = (event: KeyboardEvent) => {
      if (event.key === 'Escape') {
        event.preventDefault()
        event.stopPropagation()
        closeRef.current()
      } else if (event.key === 'ArrowRight') stepRef.current(1)
      else if (event.key === 'ArrowLeft') stepRef.current(-1)
    }
    document.addEventListener('keydown', onKey, true)
    return () => {
      document.removeEventListener('keydown', onKey, true)
      restore?.focus()
    }
  }, [])
  return createPortal(
    <div aria-label={file.name} aria-modal="true" className="aw-viewer" onClick={onClose} role="dialog">
      <div className="aw-viewer__bar" onClick={(event) => event.stopPropagation()}>
        <span className="aw-viewer__name">{file.name}</span>
        <span className="aw-viewer__meta">
          {file.width && file.height ? `${file.width} × ${file.height} · ` : ''}
          {formatBytes(file.size)}
        </span>
        <a aria-label={`Download ${file.name}`} className="aw-iconbtn" download={file.name} href={fileContentUrl(file.id, true)}>
          <Icon name={Download} />
        </a>
        <button aria-label="Close" className="aw-iconbtn" onClick={onClose} ref={closeButton} type="button">
          <Icon name="x" />
        </button>
      </div>
      <img
        alt={file.name}
        className="aw-viewer__image"
        onClick={(event) => event.stopPropagation()}
        src={fileContentUrl(file.id)}
      />
      {images.length > 1 ? (
        <>
          <button
            aria-label="Previous picture"
            className="aw-viewer__step is-previous"
            onClick={(event) => {
              event.stopPropagation()
              step(-1)
            }}
            type="button"
          >
            <Icon name="arrow-left" size={20} />
          </button>
          <button
            aria-label="Next picture"
            className="aw-viewer__step is-next"
            onClick={(event) => {
              event.stopPropagation()
              step(1)
            }}
            type="button"
          >
            <Icon name="arrow-left" size={20} />
          </button>
        </>
      ) : null}
    </div>,
    document.body,
  )
}

function hostOf(url: string) {
  try {
    return new URL(url).hostname.replace(/^www\./, '')
  } catch {
    return url
  }
}

/** Cards for the pages a message links to. The author can hide one; it stays hidden for everyone. */
export function LinkPreviews({
  conversationId,
  messageId,
  canHide,
  previews,
}: {
  conversationId?: string
  messageId: string
  canHide: boolean
  previews: LinkPreview[]
}) {
  const [hidden, setHidden] = useState<string[]>([])
  const shown = previews.filter((preview) => !hidden.includes(preview.id))
  if (shown.length === 0) return null
  const hide = (preview: LinkPreview) => {
    if (!conversationId) return
    setHidden((current) => [...current, preview.id])
    hideLinkPreview({ conversationId, messageId, previewId: preview.id }).catch(() =>
      setHidden((current) => current.filter((id) => id !== preview.id)),
    )
  }
  return (
    <div className="aw-previews">
      {shown.map((preview) => (
        <div className="aw-preview" key={preview.id}>
          <a className="aw-preview__link" href={preview.url} rel="noreferrer noopener" target="_blank">
            <span className="aw-preview__site">{preview.siteName ?? hostOf(preview.url)}</span>
            <span className="aw-preview__title">{preview.title}</span>
            {preview.description ? <span className="aw-preview__desc">{preview.description}</span> : null}
            {preview.imageWidth && preview.imageHeight ? (
              <img
                alt=""
                className="aw-preview__image"
                decoding="async"
                height={preview.imageHeight}
                loading="lazy"
                src={linkPreviewImageUrl(preview.id)}
                width={preview.imageWidth}
              />
            ) : null}
          </a>
          {canHide && conversationId ? (
            <IconButton className="aw-preview__hide" icon="x" label="Remove preview" onClick={() => hide(preview)} size="sm" />
          ) : null}
        </div>
      ))}
    </div>
  )
}
