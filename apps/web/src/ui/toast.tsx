import { useEffect, useRef, useState } from 'react'
import { createPortal } from 'react-dom'

import { Icon } from './icons'
import { TOAST_ICONS, dismissToast, showToast, useToasts, type Toast } from './toastStore'

function ToastItem({ toast }: { toast: Toast }) {
  const [paused, setPaused] = useState(false)
  const [undoing, setUndoing] = useState(false)
  const remaining = useRef(toast.duration)
  const startedAt = useRef(0)

  useEffect(() => {
    if (!toast.duration || paused) return undefined
    startedAt.current = Date.now()
    const timer = window.setTimeout(() => dismissToast(toast.id), remaining.current)
    return () => {
      window.clearTimeout(timer)
      remaining.current = Math.max(1_000, remaining.current - (Date.now() - startedAt.current))
    }
  }, [paused, toast.duration, toast.id, toast.createdAt])

  const undo = async () => {
    if (!toast.onUndo || undoing) return
    setUndoing(true)
    try {
      await toast.onUndo()
      dismissToast(toast.id)
    } catch (reason) {
      showToast({
        id: toast.id,
        tone: 'danger',
        title: 'Couldn’t undo that.',
        description: reason instanceof Error ? reason.message : 'Try again in a moment.',
      })
    }
  }

  return (
    <li
      className={`aw-toast aw-toast--${toast.tone}`}
      onBlur={() => setPaused(false)}
      onFocus={() => setPaused(true)}
      onMouseEnter={() => setPaused(true)}
      onMouseLeave={() => setPaused(false)}
      role={toast.tone === 'danger' ? 'alert' : 'status'}
    >
      <Icon className="aw-toast__icon" name={TOAST_ICONS[toast.tone]} size={18} />
      <div className="aw-toast__text">
        <p className="aw-toast__title">{toast.title}</p>
        {toast.description ? <p className="aw-toast__description">{toast.description}</p> : null}
      </div>
      <div className="aw-toast__actions">
        {toast.onUndo ? (
          <button className="aw-toast__action" disabled={undoing} onClick={() => void undo()} type="button">
            {undoing ? 'Undoing…' : 'Undo'}
          </button>
        ) : null}
        {toast.action ? (
          <button
            className="aw-toast__action"
            onClick={() => {
              dismissToast(toast.id)
              toast.action?.onSelect()
            }}
            type="button"
          >
            {toast.action.label}
          </button>
        ) : null}
        <button aria-label="Dismiss" className="aw-toast__close" onClick={() => dismissToast(toast.id)} type="button">
          <Icon name="x" size={14} />
        </button>
      </div>
    </li>
  )
}

/** Where toasts appear. Render once, near the root. */
export function ToastViewport() {
  const current = useToasts()
  if (typeof document === 'undefined') return null
  return createPortal(
    <section aria-label="Notifications" aria-live="polite" aria-relevant="additions" className="aw-toasts">
      <ol className="aw-toasts__list">
        {current.map((toast) => (
          <ToastItem key={`${toast.id}:${toast.createdAt}`} toast={toast} />
        ))}
      </ol>
    </section>,
    document.body,
  )
}
