import type { MouseEvent, ReactNode } from 'react'
import { Link } from 'react-router-dom'

import { Button } from '../ui'
import './gallery.css'

// The gallery piece (W1.20): a card for each item, in the owner's order,
// with its title and the values worth showing, and a picture across the top
// when the item has one. A card opens its item; more load with a button at
// the end. Cards flow into as many columns as fit.

export type GalleryPieceProps<T extends { id: string }> = {
  label: string
  items: readonly T[]
  title: (item: T) => string
  href: (item: T) => string
  onOpen?: (item: T) => void
  openId?: string | null
  /** Beside the title, such as a number. */
  badge?: (item: T) => ReactNode
  /** Under the title: the card's values. */
  details?: (item: T) => ReactNode
  /** Across the top of the card: a picture of the item, such as a whiteboard's drawing. */
  cover?: (item: T) => ReactNode
  /** Beside the title: the card's own menu. */
  actions?: (item: T) => ReactNode
  /** Above the cards: a count, a way to add one. */
  header?: ReactNode
  /** Shown instead of the cards when there are none. */
  empty?: ReactNode
  hasMore?: boolean
  loadingMore?: boolean
  /** Loading more failed: the button says to try again. */
  moreFailed?: boolean
  onLoadMore?: () => void
}

export function GalleryPiece<T extends { id: string }>(props: GalleryPieceProps<T>) {
  const { items } = props
  return (
    <div className="aw-gallery">
      {props.header}
      {items.length === 0 ? (
        props.empty
      ) : (
        <ul aria-label={props.label} className="aw-gallery__grid">
          {items.map((item) => (
            <li aria-current={item.id === props.openId || undefined} className="aw-gallery__card" data-part="gallery-card" key={item.id}>
              {props.cover ? (
                // The title is the card's link; the picture is a bigger place to click, hidden from screen readers.
                <ItemLink aria-hidden className="aw-gallery__cover" href={props.href(item)} onOpen={props.onOpen && (() => props.onOpen?.(item))} tabIndex={-1}>
                  {props.cover(item)}
                </ItemLink>
              ) : null}
              <div className="aw-gallery__body">
                <span className="aw-gallery__head">
                  <ItemLink className="aw-gallery__title" href={props.href(item)} onOpen={props.onOpen && (() => props.onOpen?.(item))}>
                    {props.title(item)}
                  </ItemLink>
                  {props.badge ? <span className="aw-gallery__badge">{props.badge(item)}</span> : null}
                  {props.actions ? <span className="aw-gallery__actions">{props.actions(item)}</span> : null}
                </span>
                {props.details?.(item)}
              </div>
            </li>
          ))}
        </ul>
      )}
      {props.hasMore ? (
        <Button busy={props.loadingMore} className="aw-gallery__more" onClick={props.onLoadMore}>
          {props.moreFailed ? 'Try loading more again' : 'Show more'}
        </Button>
      ) : null}
    </div>
  )
}

/** A link to an item: in the app it moves without reloading, and `onOpen` (a side panel) takes a plain click. */
function ItemLink({
  children,
  className,
  href,
  onOpen,
  tabIndex,
  ...aria
}: {
  children: ReactNode
  className: string
  href: string
  onOpen?: () => void
  tabIndex?: number
  'aria-hidden'?: boolean
}) {
  const onClick = (event: MouseEvent<HTMLAnchorElement>) => {
    if (event.metaKey || event.ctrlKey || event.shiftKey || !onOpen) return
    event.preventDefault()
    onOpen()
  }
  if (href.startsWith('/')) {
    return (
      <Link {...aria} className={className} onClick={onClick} tabIndex={tabIndex} to={href}>
        {children}
      </Link>
    )
  }
  return (
    <a {...aria} className={className} href={href} onClick={onClick} rel="noreferrer" tabIndex={tabIndex} target="_blank">
      {children}
    </a>
  )
}
