import { Children, isValidElement, useId, useState, type ReactElement, type ReactNode } from 'react'
import { Link, NavLink } from 'react-router-dom'

import { Avatar, Badge, Dots, Icon, IconButton, Menu, type ActorKind, type IconName, type MenuEntry } from '../ui'

/** The sidebar column: a header with the space name and its menu, then the scrolling list. */
export function SidebarFrame({
  actions,
  children,
  id,
  label,
  menu,
  mobileOpen,
  title,
}: {
  actions?: ReactNode
  children: ReactNode
  id: string
  label: string
  menu?: MenuEntry[]
  mobileOpen: boolean
  title: string
}) {
  return (
    <aside aria-label={label} className="aw-sidebar" data-mobile-open={mobileOpen} data-region="sidebar" id={id}>
      <header className="aw-sidebar__header">
        {menu ? (
          <Menu
            items={menu}
            label={`${title} menu`}
            trigger={
              <button className="aw-sidebar__title" type="button">
                <span>{title}</span>
                <Icon name="chevron-down" />
              </button>
            }
          />
        ) : (
          <h2 className="aw-sidebar__title aw-sidebar__title--static">
            <span>{title}</span>
          </h2>
        )}
        {actions ? <div className="aw-sidebar__actions">{actions}</div> : null}
      </header>
      <div className="aw-sidebar__scroll">{children}</div>
    </aside>
  )
}

type RowProps = { keep?: boolean }

/** A collapsible group. Collapsed, it still shows the current row and anything that needs you. */
export function SidebarSection({
  addLabel,
  children,
  label,
  onAdd,
}: {
  addLabel?: string
  children: ReactNode
  label: string
  onAdd?: () => void
}) {
  const [open, setOpen] = useState(true)
  const id = useId()
  const rows = Children.toArray(children).filter(
    (child) => open || (isValidElement(child) && (child as ReactElement<RowProps>).props.keep),
  )
  return (
    <section className="aw-section">
      <div className="aw-section__head">
        <button aria-controls={id} aria-expanded={open} className="aw-section__toggle" onClick={() => setOpen(!open)} type="button">
          <Icon className="aw-section__caret" name="chevron-down" size={14} />
          <span>{label}</span>
        </button>
        {onAdd ? <IconButton className="aw-section__add" icon="plus" label={addLabel ?? `Add to ${label}`} onClick={onAdd} size="sm" /> : null}
      </div>
      <ul className="aw-section__list" id={id}>
        {rows}
      </ul>
    </section>
  )
}

export type SidebarAttention = 'needs-you' | 'working' | 'failed' | 'done'

function AttentionMark({ kind }: { kind: SidebarAttention }) {
  if (kind === 'needs-you') return <span aria-hidden="true" className="aw-attn aw-attn--needs">Needs you</span>
  if (kind === 'working') return <Dots />
  if (kind === 'failed') {
    return (
      <span aria-hidden="true" className="aw-attn aw-attn--failed">
        <Icon name="triangle-alert" size={12} />
      </span>
    )
  }
  return (
    <span aria-hidden="true" className="aw-attn aw-attn--done">
      <Icon name="check" size={14} />
    </span>
  )
}

/** One row: a link with an icon or avatar, bold when unread, with optional state on the right. */
export function SidebarLink({
  active,
  attention,
  attentionLabel,
  avatar,
  badge,
  dataAttention,
  end,
  icon,
  label,
  meta,
  nested = false,
  onNavigate,
  to,
  unread = false,
}: {
  /** Marks the row current for pages it holds besides its own, such as Work items on Milestones. */
  active?: boolean
  attention?: SidebarAttention
  /** Spoken state, for example "Needs your response". */
  attentionLabel?: string
  avatar?: { name: string; kind: ActorKind; seed?: string }
  badge?: number
  dataAttention?: string
  end?: boolean
  icon?: IconName
  keep?: boolean
  label: string
  meta?: string
  nested?: boolean
  onNavigate?: () => void
  to: string
  unread?: boolean
}) {
  const rowClass = (isActive: boolean) =>
    ['aw-item', isActive ? 'is-active' : '', unread || badge ? 'is-unread' : '', nested ? 'aw-item--nested' : ''].filter(Boolean).join(' ')
  const content = (
    <>
      {nested ? null : (
        <span className="aw-item__lead">
          {avatar ? <Avatar kind={avatar.kind} name={avatar.name} seed={avatar.seed} size={20} /> : icon ? <Icon name={icon} /> : null}
        </span>
      )}
      <span className="aw-item__label">{label}</span>
      {attentionLabel ? <span className="visually-hidden">, {attentionLabel}</span> : null}
      {attention || badge || meta ? (
        <span className="aw-item__trail">
          {attention ? <AttentionMark kind={attention} /> : null}
          {badge ? <Badge count={badge} /> : null}
          {meta ? <span className="aw-item__meta">{meta}</span> : null}
        </span>
      ) : null}
    </>
  )
  return (
    <li data-attention={dataAttention}>
      {/* NavLink only marks its own path current, so a row that holds other pages marks itself. */}
      {active === undefined ? (
        <NavLink className={({ isActive }) => rowClass(isActive)} end={end} onClick={onNavigate} to={to}>
          {content}
        </NavLink>
      ) : (
        <Link aria-current={active ? 'page' : undefined} className={rowClass(active)} onClick={onNavigate} to={to}>
          {content}
        </Link>
      )}
    </li>
  )
}
