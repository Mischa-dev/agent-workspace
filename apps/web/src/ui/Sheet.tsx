import { useEffect, useId, useRef, type ReactNode } from 'react'

import { IconButton } from './controls'
import { inOwnControl } from './focusRule'
import { useReturnFocus } from './useReturnFocus'

const FOCUSABLE =
  'a[href], button:not([disabled]), input:not([disabled]), select:not([disabled]), textarea:not([disabled]), [tabindex]:not([tabindex="-1"]), [contenteditable="true"]'

/**
 * A bottom sheet for phones: a drag handle, a title and a close button.
 * Focus stays inside while it's open (Tab wraps), Escape closes it unless a
 * field or menu in it has the key, and focus goes back where it was.
 */
export function Sheet({
  children,
  label,
  onClose,
  title,
}: {
  children: ReactNode
  /** The accessible name when there's no visible title. */
  label?: string
  onClose: () => void
  title?: ReactNode
}) {
  const ref = useRef<HTMLDivElement>(null)
  const titleId = useId()
  useReturnFocus(ref)
  useEffect(() => {
    const sheet = ref.current
    if (!sheet) return
    if (!sheet.contains(document.activeElement)) {
      ;(sheet.querySelector<HTMLElement>(FOCUSABLE) ?? sheet).focus()
    }
  }, [])
  return (
    <div className="aw-k-sheet-layer">
      <div aria-hidden="true" className="aw-k-sheet-backdrop" onClick={onClose} />
      <div
        aria-label={title ? undefined : label}
        aria-labelledby={title ? titleId : undefined}
        aria-modal="true"
        className="aw-k-sheet"
        onKeyDown={(event) => {
          if (event.key === 'Escape' && !inOwnControl(event.target)) {
            event.stopPropagation()
            onClose()
            return
          }
          if (event.key !== 'Tab') return
          const focusable = [...(ref.current?.querySelectorAll<HTMLElement>(FOCUSABLE) ?? [])]
          if (!focusable.length) return
          const first = focusable[0]
          const last = focusable[focusable.length - 1]
          if (event.shiftKey && document.activeElement === first) {
            event.preventDefault()
            last.focus()
          } else if (!event.shiftKey && document.activeElement === last) {
            event.preventDefault()
            first.focus()
          }
        }}
        ref={ref}
        role="dialog"
        tabIndex={-1}
      >
        <div aria-hidden="true" className="aw-k-sheet__handle" />
        <div className="aw-k-sheet__head">
          {title ? (
            <h2 className="aw-k-sheet__title" id={titleId}>
              {title}
            </h2>
          ) : null}
          <IconButton icon="x" label="Close" onClick={onClose} size="sm" tooltip={false} />
        </div>
        <div className="aw-k-sheet__body">{children}</div>
      </div>
    </div>
  )
}
