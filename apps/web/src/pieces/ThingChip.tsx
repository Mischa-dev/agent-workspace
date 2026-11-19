import { useId, useRef, useState, type ReactNode } from 'react'
import { Link } from 'react-router-dom'

import { Icon, type IconName } from '../ui'
import './detail.css'

// A chip for a thing named inside another (W1.21): its icon and name, a
// link when it has an address, and on hover or focus a small card with what
// kind of thing it is and anything the owner adds. The card only describes;
// everything it says is also on the thing itself.

const KIND: Record<string, { word: string; icon: IconName }> = {
  task: { word: 'Task', icon: 'checklist' },
  page: { word: 'Page', icon: 'list' },
  record: { word: 'Record', icon: 'table' },
  person: { word: 'Person', icon: 'users' },
  whiteboard: { word: 'Whiteboard', icon: 'whiteboard' },
  thread: { word: 'Thread', icon: 'messages' },
  channel: { word: 'Channel', icon: 'hash' },
}

const SHOW_AFTER_MS = 400

export function ThingChip({ details, href, kind, label }: { details?: ReactNode; href?: string; kind: string; label: string }) {
  const known = KIND[kind] ?? { word: kind.charAt(0).toUpperCase() + kind.slice(1), icon: 'link' as IconName }
  const [open, setOpen] = useState(false)
  const timer = useRef<number | undefined>(undefined)
  const card = useId()
  const show = (now: boolean) => {
    window.clearTimeout(timer.current)
    if (now) setOpen(true)
    else timer.current = window.setTimeout(() => setOpen(true), SHOW_AFTER_MS)
  }
  const hide = () => {
    window.clearTimeout(timer.current)
    setOpen(false)
  }
  const face = (
    <>
      <Icon name={known.icon} size={12} />
      <span className="aw-thingchip__label">{label}</span>
    </>
  )
  const shared = {
    'aria-describedby': open ? card : undefined,
    className: 'aw-thingchip',
    onBlur: hide,
    onFocus: () => show(true),
    onKeyDown: (event: { key: string }) => {
      if (event.key === 'Escape') hide()
    },
    onMouseEnter: () => show(false),
    onMouseLeave: hide,
  }
  return (
    <span className="aw-thingchip__wrap">
      {href ? (
        <Link {...shared} onClick={(event) => event.stopPropagation()} to={href}>
          {face}
        </Link>
      ) : (
        // Without an address it may sit inside a button (a table cell), so it
        // takes no focus of its own: the card shows on hover only.
        <span {...shared}>{face}</span>
      )}
      {open ? (
        <span className="aw-thingchip__card" id={card} role="tooltip">
          <span className="aw-thingchip__kind">{known.word}</span>
          <span className="aw-thingchip__title">{label}</span>
          {details ? <span className="aw-thingchip__details">{details}</span> : null}
        </span>
      ) : null}
    </span>
  )
}
