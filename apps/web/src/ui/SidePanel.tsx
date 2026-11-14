import { useId, useRef, type ReactNode } from 'react'

import { IconButton } from './controls'
import { inOwnControl, overlayOpen } from './focusRule'
import { Sheet } from './Sheet'
import { usePhoneWidth } from './useCoarsePointer'
import { useReturnFocus } from './useReturnFocus'

/**
 * A panel beside the page: a title, a close button and its contents. Escape
 * closes it unless a field, menu or dialog has the key (the one shared rule,
 * `focusRule.ts`), and focus goes back where it was before it opened. On a
 * phone it opens as a bottom sheet instead.
 */
export function SidePanel({
  actions,
  children,
  className,
  narrow,
  onClose,
  title,
}: {
  /** Buttons beside the title. */
  actions?: ReactNode
  children: ReactNode
  className?: string
  /** Force the narrow (sheet) or wide form; by default it follows the screen. */
  narrow?: boolean
  onClose: () => void
  title: ReactNode
}) {
  const phone = usePhoneWidth()
  if (narrow ?? phone) {
    return <Sheet onClose={onClose} title={title}>{children}</Sheet>
  }
  return (
    <WidePanel actions={actions} className={className} onClose={onClose} title={title}>
      {children}
    </WidePanel>
  )
}

function WidePanel({
  actions,
  children,
  className,
  onClose,
  title,
}: {
  actions?: ReactNode
  children: ReactNode
  className?: string
  onClose: () => void
  title: ReactNode
}) {
  const ref = useRef<HTMLElement>(null)
  const titleId = useId()
  useReturnFocus(ref)
  return (
    <aside
      aria-labelledby={titleId}
      className={['aw-k-sidepanel', className].filter(Boolean).join(' ')}
      onKeyDown={(event) => {
        if (event.key !== 'Escape' || event.defaultPrevented) return
        if (inOwnControl(event.target) || overlayOpen()) return
        event.preventDefault()
        onClose()
      }}
      ref={ref}
    >
      <div className="aw-k-sidepanel__head">
        <h2 className="aw-k-sidepanel__title" id={titleId}>
          {title}
        </h2>
        {actions ? <div className="aw-k-sidepanel__actions">{actions}</div> : null}
        <IconButton icon="x" label="Close panel" onClick={onClose} size="sm" />
      </div>
      <div className="aw-k-sidepanel__body">{children}</div>
    </aside>
  )
}
