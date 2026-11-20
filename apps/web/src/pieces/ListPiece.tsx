import type { AnchorHTMLAttributes, ReactNode } from 'react'
import { Link } from 'react-router-dom'

import './list.css'

// The list piece (W1.28): one row for each item, in the owner's order. A row
// has a mark at the start, a title with an optional line under it, and at the
// end the values worth a glance and the row's own actions. Every list in the
// app draws its rows here (My work, Later, decisions, scheduled messages), so
// a fix to how rows look, wrap on a phone or take focus is made once.
//
// The piece doesn't fetch or decide anything: the page passes its items and
// says what each part of a row shows.

export type ListPieceProps<T> = {
  /** What the list is called, for screen readers; or the id of its heading. */
  label?: string
  labelledBy?: string
  items: readonly T[]
  itemKey: (item: T) => string
  /** The row's title: a `ListPieceTitle`, which links where the item opens. */
  title: (item: T) => ReactNode
  /** A mark before the title: an icon, a status. */
  leading?: (item: T) => ReactNode
  /** A line under the title: where it is from, when, a short preview. */
  meta?: (item: T) => ReactNode
  /** At the end: chips and values worth a glance. */
  aside?: (item: T) => ReactNode
  /** At the very end: the row's own buttons and menus. */
  actions?: (item: T) => ReactNode
  /** The item open beside the list, such as in the side panel. */
  isOpen?: (item: T) => boolean
  /** A finished item reads quieter, its title struck through. */
  isDone?: (item: T) => boolean
  /** The whole row opens the item, not just its title (rows without actions). */
  wholeRow?: boolean
  /** Compact rows are one line high; comfortable ones leave room for the line under the title. */
  density?: 'compact' | 'comfortable'
  /** Shown instead of the list when there are no items. */
  empty?: ReactNode
  /** The row's own id, so a link can point at it (`#decision-<id>`). */
  rowId?: (item: T) => string
  className?: string
}

export function ListPiece<T>({
  actions,
  aside,
  className,
  density = 'comfortable',
  empty = null,
  isDone,
  isOpen,
  itemKey,
  items,
  label,
  labelledBy,
  leading,
  meta,
  rowId,
  title,
  wholeRow = false,
}: ListPieceProps<T>) {
  if (items.length === 0) return <>{empty}</>
  // Built on the kit's row list (`.aw-rows`, `.aw-row`), so a piece's rows and the app's own row lists are one surface.
  const classes = ['aw-rows', 'aw-list-piece', `aw-list-piece--${density}`, className ?? '']
  return (
    <ul aria-label={labelledBy ? undefined : label} aria-labelledby={labelledBy} className={classes.filter(Boolean).join(' ')} data-piece="list">
      {items.map((item) => {
        const lead = leading?.(item)
        const under = meta?.(item)
        const glance = aside?.(item)
        const own = actions?.(item)
        const rowClasses = ['aw-row', 'aw-list-piece__row', wholeRow ? 'aw-row--link' : '', isOpen?.(item) ? 'is-open' : '', isDone?.(item) ? 'is-done' : '']
        return (
          <li className={rowClasses.filter(Boolean).join(' ')} data-part="list-row" id={rowId?.(item)} key={itemKey(item)}>
            {lead ? <span className="aw-row__lead">{lead}</span> : null}
            <div className="aw-row__main">
              <div className="aw-row__title">{title(item)}</div>
              {under ? <div className="aw-row__meta">{under}</div> : null}
            </div>
            {glance ? <div className="aw-list-piece__aside">{glance}</div> : null}
            {own ? <div className="aw-row__actions">{own}</div> : null}
          </li>
        )
      })}
    </ul>
  )
}

type TitleProps = Omit<AnchorHTMLAttributes<HTMLAnchorElement>, 'href' | 'className'> & {
  /** Where the item opens in the app. Without it, the title is plain text: the item has nowhere to open. */
  to?: string
  /** Marks the row open beside the list. */
  current?: boolean
  children: ReactNode
}

/** A row's title. The list's keyboard walk (`useListKeys` on `.aw-list-piece__title`) moves between these. */
export function ListPieceTitle({ children, current, to, ...rest }: TitleProps) {
  if (!to) return <span className="aw-list-piece__title">{children}</span>
  // `aw-row__link` lets a whole-row list's link cover the row.
  return (
    <Link aria-current={current || undefined} className="aw-list-piece__title aw-row__link" to={to} {...rest}>
      {children}
    </Link>
  )
}

/** The selector `useListKeys` walks in a list piece. */
export const LIST_PIECE_TITLE = '.aw-list-piece__title'
