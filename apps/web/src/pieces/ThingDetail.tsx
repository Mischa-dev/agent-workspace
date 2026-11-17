import { useId, useState, type KeyboardEvent, type ReactNode } from 'react'

import { IconButton } from '../ui'
import { usePhoneWidth } from '../ui/useCoarsePointer'
import './detail.css'

// The detail view (W1.21): one thing beside its list (a panel) or on its
// own (a page). A bar names where it is from and holds its actions; on a
// phone the panel is a whole-screen sheet whose bar starts with Back. What
// it holds is the owner's: sections in order, or tabs (fields, discussion,
// history) when there's too much for one scroll. Escape closes a panel
// unless a field or a picker in it wants the key.

export type ThingDetailTab = { id: string; label: string; content: ReactNode }

export function ThingDetail({
  actions,
  as = 'panel',
  busy = false,
  children,
  eyebrow,
  label,
  onClose,
  tabs,
}: {
  /** In the bar, before Close: a menu, a button. */
  actions?: ReactNode
  as?: 'panel' | 'page'
  /** Still loading: said to assistive technology. */
  busy?: boolean
  /** What the detail holds, above any tabs. */
  children?: ReactNode
  /** Where the thing is from: its collection and number, its space. */
  eyebrow?: ReactNode
  /** What a screen reader calls the detail: the thing's name. */
  label: string
  /** A panel's Close (Back on a phone); a page has none. */
  onClose?: () => void
  tabs?: readonly ThingDetailTab[]
}) {
  const phone = usePhoneWidth()
  const sheet = as === 'panel' && phone
  const Box = as === 'panel' ? 'aside' : 'article'
  const onKeyDown = (event: KeyboardEvent<HTMLElement>) => {
    if (as !== 'panel' || !onClose || event.key !== 'Escape' || event.defaultPrevented) return
    if ((event.target as HTMLElement).closest('input, textarea, select, .aw-popover, [role="dialog"][aria-modal="true"]')) return
    event.preventDefault()
    onClose()
  }
  return (
    <Box
      aria-busy={busy || undefined}
      aria-label={label}
      className={['aw-detail', `aw-detail--${as}`, sheet ? 'is-sheet' : ''].filter(Boolean).join(' ')}
      data-piece="detail"
      onKeyDown={onKeyDown}
    >
      <div className="aw-detail__bar">
        {sheet && onClose ? <IconButton icon="arrow-left" label="Back" onClick={onClose} size="sm" /> : null}
        <span className="aw-detail__eyebrow">{eyebrow}</span>
        {actions}
        {!sheet && onClose ? <IconButton icon="x" label="Close" onClick={onClose} size="sm" /> : null}
      </div>
      {children}
      {tabs?.length ? <DetailTabs tabs={tabs} /> : null}
    </Box>
  )
}

/** Tabs under the detail's head: arrow keys move between them. */
function DetailTabs({ tabs }: { tabs: readonly ThingDetailTab[] }) {
  const [chosen, setChosen] = useState(tabs[0]?.id)
  const base = useId()
  const current = tabs.find((tab) => tab.id === chosen) ?? tabs[0]
  const move = (event: KeyboardEvent<HTMLButtonElement>, index: number) => {
    const step = event.key === 'ArrowRight' ? 1 : event.key === 'ArrowLeft' ? -1 : 0
    if (!step) return
    event.preventDefault()
    const next = tabs[(index + step + tabs.length) % tabs.length]
    setChosen(next.id)
    document.getElementById(`${base}-tab-${next.id}`)?.focus()
  }
  return (
    <div className="aw-detail__tabs">
      <div className="aw-detail__tablist" role="tablist">
        {tabs.map((tab, index) => (
          <button
            aria-controls={`${base}-panel-${tab.id}`}
            aria-selected={tab.id === current.id}
            className="aw-detail__tab"
            id={`${base}-tab-${tab.id}`}
            key={tab.id}
            onClick={() => setChosen(tab.id)}
            onKeyDown={(event) => move(event, index)}
            role="tab"
            tabIndex={tab.id === current.id ? 0 : -1}
            type="button"
          >
            {tab.label}
          </button>
        ))}
      </div>
      <div aria-labelledby={`${base}-tab-${current.id}`} className="aw-detail__tabpanel" id={`${base}-panel-${current.id}`} role="tabpanel">
        {current.content}
      </div>
    </div>
  )
}
