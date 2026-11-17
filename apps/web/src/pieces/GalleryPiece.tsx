import type { ReactNode } from 'react'

import { Button } from '../ui'
import './gallery.css'

// The gallery piece (W1.20): a card for each item, in the owner's order,
// with its title and the values worth showing. A card opens its item; more
// load with a button at the end. Cards flow into as many columns as fit.

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
            <li aria-current={item.id === props.openId || undefined} className="aw-gallery__card" key={item.id}>
              <span className="aw-gallery__head">
                <a
                  className="aw-gallery__title"
                  href={props.href(item)}
                  onClick={(event) => {
                    if (event.metaKey || event.ctrlKey || event.shiftKey || !props.onOpen) return
                    event.preventDefault()
                    props.onOpen(item)
                  }}
                >
                  {props.title(item)}
                </a>
                {props.badge ? <span className="aw-gallery__badge">{props.badge(item)}</span> : null}
              </span>
              {props.details?.(item)}
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
