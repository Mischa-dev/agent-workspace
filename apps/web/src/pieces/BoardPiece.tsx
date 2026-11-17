import {
  useCallback,
  useEffect,
  useLayoutEffect,
  useMemo,
  useRef,
  useState,
  type ComponentType,
  type CSSProperties,
  type KeyboardEvent as ReactKeyboardEvent,
  type PointerEvent as ReactPointerEvent,
  type ReactNode,
  type RefObject,
} from 'react'

import { Button, IconButton, ListSkeleton, Menu, SelectField, showToast, type MenuEntry } from '../ui'
import { useBoardDrag, type DropTarget } from '../ui/useBoardDrag'
import './board.css'

// The board piece (W1.19): columns of cards, each column loading its own
// cards a page at a time. Cards move between columns by dragging (a mouse,
// or a finger after a moment's hold), by the keyboard (Space picks a card
// up, the arrows carry it, Space puts it down, Escape puts it back) and by
// each card's Move to menu. A moved card shows in its new column at once; a
// move the owner refuses goes back, with the reason. Narrow, the board shows
// one column at a time with a switcher. What a column holds and how a move
// saves are the owner's: collections today, the task board with T1.

/** A column: its key, what it's called and how many cards it holds in all. */
export type BoardColumn = { key: string; label: string; color?: string | null; count: number }

/** One column's cards, as its source loads them. */
export type BoardColumnState<T> = {
  items: readonly T[]
  /** Nothing has loaded yet. */
  pending: boolean
  /** Nothing loaded, and the load failed. */
  failed: boolean
  onRetry: () => void
  hasMore: boolean
  loadingMore: boolean
  onLoadMore: () => void
  /** When the newest load that worked began (ms): a card moved here before then is in it, or no longer belongs. */
  fetchedFrom: number
}

/** Loads one column's cards and hands them to `children`. Rendered once per column shown, so it can use hooks. */
export type BoardColumnSource<T, S> = ComponentType<{ column: BoardColumn; source: S; children: (state: BoardColumnState<T>) => ReactNode }>

/** A move's answer: true when it saved; false (the owner already said why) or a reason when it didn't, and the card goes back. */
export type BoardMoveResult = true | false | string

export type BoardPieceProps<T extends { id: string }, S> = {
  label: string
  columns: readonly BoardColumn[]
  Source: BoardColumnSource<T, S>
  /** What `Source` needs besides the column. */
  source: S
  /** The column a card is in now. */
  columnOf: (item: T) => string
  /** The card as it shows in another column before the move saves. */
  movedTo: (item: T, key: string) => T
  /** Moves a card; without it, cards stay put. */
  onMove?: (item: T, to: string) => Promise<BoardMoveResult>
  /** What the Move to menu says it changes, such as a field's name. */
  movesBy?: string
  title: (item: T) => string
  href: (item: T) => string
  onOpen?: (item: T) => void
  openId?: string | null
  /** Beside the title, such as a number. */
  badge?: (item: T) => ReactNode
  /** Under the title: the card's values. */
  details?: (item: T) => ReactNode
  /** Above the board: a count, a clear-filters button. */
  summary?: ReactNode
}

/** A card moved to another column, shown there until the column loads it, or reloads after the save. */
type Moved<T> = { item: T; key: string; at: number; savedAt?: number }

/** Keyboard pick-up: the card, where it was and where it would go. */
type Held = { id: string; from: string; to: string }

/** Below this width the board shows one column at a time. */
const NARROW_BELOW = 560

function useNarrow(root: RefObject<HTMLElement | null>) {
  const [narrow, setNarrow] = useState(() => typeof window !== 'undefined' && window.innerWidth < NARROW_BELOW)
  useLayoutEffect(() => {
    const element = root.current
    if (!element || typeof ResizeObserver === 'undefined') return undefined
    const observer = new ResizeObserver(([entry]) => setNarrow(entry.contentRect.width < NARROW_BELOW))
    observer.observe(element)
    return () => observer.disconnect()
  }, [root])
  return narrow
}

const columnName = (column: BoardColumn) => column.label || 'No value'

const sameCards = <T,>(before: readonly T[] | undefined, after: readonly T[]) =>
  before !== undefined && before.length === after.length && before.every((item, index) => item === after[index])

export function BoardPiece<T extends { id: string }, S>(props: BoardPieceProps<T, S>) {
  const { columns, columnOf, label, movedTo, onMove, summary } = props
  const movable = Boolean(onMove)
  const [moved, setMoved] = useState<Moved<T>[]>([])
  const [held, setHeld] = useState<Held | null>(null)
  const [said, setSaid] = useState('')
  const rootRef = useRef<HTMLDivElement>(null)
  const scrollerRef = useRef<HTMLDivElement>(null)
  const ghostRef = useRef<HTMLDivElement>(null)
  const narrow = useNarrow(rootRef)
  const [shownColumn, setShownColumn] = useState(columns[0]?.key ?? '')

  // Each column's cards as it shows them, to find the one a drop names.
  const [shownCards, setShownCards] = useState<Record<string, readonly T[]>>({})
  const onCards = useCallback(
    (key: string, cards: readonly T[]) => setShownCards((current) => (sameCards(current[key], cards) ? current : { ...current, [key]: cards })),
    [],
  )
  const findCard = (id: string) => Object.values(shownCards).flat().find((item) => item.id === id)
  // A moved card stays in its new column until that column's own list has
  // it, or has loaded again since the save landed (then the list is the
  // truth: the card may sort past its loaded page, or be gone). Not by a
  // timer: a save can patch every list before the destination loads (#244).
  const arrived = useCallback((id: string) => setMoved((current) => current.filter((entry) => entry.item.id !== id)), [])

  const move = (item: T, key: string) => {
    if (!onMove || columnOf(item) === key) return
    const at = Date.now()
    setMoved((current) => [...current.filter((entry) => entry.item.id !== item.id), { item: movedTo(item, key), key, at }])
    void onMove(item, key).then(
      (result) => {
        const now = Date.now()
        setMoved((current) =>
          result === true
            ? current.map((entry) => (entry.item.id === item.id && entry.at === at ? { ...entry, savedAt: now } : entry))
            : current.filter((entry) => entry.item.id !== item.id || entry.at !== at),
        )
        if (typeof result === 'string') {
          setSaid(`${props.title(item)} went back: ${result}`)
          showToast({ tone: 'danger', title: `${props.title(item)} didn’t move`, description: result })
        }
      },
      () => setMoved((current) => current.filter((entry) => entry.item.id !== item.id || entry.at !== at)),
    )
  }

  const drag = useBoardDrag({
    enabled: movable,
    ghostRef,
    scrollerRef,
    onDrop: (id: string, target: DropTarget) => {
      const item = findCard(id)
      if (item) move(item, target.columnKey)
    },
  })
  const dragged = drag.drag ? findCard(drag.drag.cardId) : undefined

  // The keyboard's pick-up: Space, then Left and Right, then Space; Escape puts it back.
  const onCardKey = (event: ReactKeyboardEvent<HTMLElement>, item: T) => {
    if (!movable || event.altKey || event.ctrlKey || event.metaKey) return
    if ((event.target as HTMLElement).closest('button, input, select, textarea, [role="menu"]')) return
    const name = props.title(item)
    if (event.key === ' ' || event.key === 'Enter') {
      if (!held && event.key === 'Enter') return
      event.preventDefault()
      if (!held || held.id !== item.id) {
        const from = columnOf(item)
        setHeld({ id: item.id, from, to: from })
        setSaid(`Picked up ${name}. Left and right arrows choose a column, Space puts it down, Escape puts it back.`)
        return
      }
      setHeld(null)
      const to = columns.find((column) => column.key === held.to)
      if (held.to !== held.from && to) {
        move(item, held.to)
        setSaid(`Moved ${name} to ${columnName(to)}.`)
      } else setSaid(`Put ${name} back.`)
      return
    }
    if (!held || held.id !== item.id) return
    if (event.key === 'Escape') {
      event.preventDefault()
      event.stopPropagation()
      setHeld(null)
      setSaid(`Put ${name} back.`)
      return
    }
    if (event.key === 'ArrowLeft' || event.key === 'ArrowRight') {
      event.preventDefault()
      const index = columns.findIndex((column) => column.key === held.to)
      const next = columns[Math.min(columns.length - 1, Math.max(0, index + (event.key === 'ArrowRight' ? 1 : -1)))]
      if (!next) return
      setHeld({ ...held, to: next.key })
      setSaid(`Over ${columnName(next)}.`)
    }
  }

  // Narrow: the switcher scrolls to a column; scrolling the board moves the switcher.
  const showColumn = (key: string) => {
    setShownColumn(key)
    const element = scrollerRef.current?.querySelector<HTMLElement>(`[data-column="${CSS.escape(key)}"]`)
    if (element && scrollerRef.current) scrollerRef.current.scrollTo({ left: element.offsetLeft - scrollerRef.current.offsetLeft, behavior: 'smooth' })
  }
  // Narrow, the board opens on the first column that has cards.
  const opened = useRef(false)
  useEffect(() => {
    if (!narrow || opened.current || !columns.length) return
    opened.current = true
    const first = columns.find((column) => column.count > 0)
    const element = first && scrollerRef.current?.querySelector<HTMLElement>(`[data-column="${CSS.escape(first.key)}"]`)
    if (!first || !element || !scrollerRef.current) return
    setShownColumn(first.key)
    scrollerRef.current.scrollLeft = element.offsetLeft - scrollerRef.current.offsetLeft
  }, [columns, narrow])
  useEffect(() => {
    const scroller = scrollerRef.current
    if (!narrow || !scroller) return undefined
    const onScroll = () => {
      const index = Math.round(scroller.scrollLeft / Math.max(1, scroller.clientWidth))
      const key = columns[Math.min(columns.length - 1, Math.max(0, index))]?.key
      if (key) setShownColumn(key)
    }
    scroller.addEventListener('scroll', onScroll, { passive: true })
    return () => scroller.removeEventListener('scroll', onScroll)
  }, [columns, narrow])
  const target = drag.drag?.target?.columnKey ?? held?.to ?? null

  return (
    <div className={narrow ? 'aw-pboard-wrap is-narrow' : 'aw-pboard-wrap'} ref={rootRef}>
      {summary || narrow ? (
        <div className="aw-pboard__top">
          {summary}
          {narrow && columns.length > 1 ? (
            <SelectField
              className="aw-pboard__switcher"
              compact
              label="Column"
              onChange={(event) => showColumn(event.target.value)}
              value={columns.some((column) => column.key === shownColumn) ? shownColumn : columns[0]?.key}
            >
              {columns.map((column) => (
                <option key={column.key} value={column.key}>
                  {columnName(column)} ({column.count})
                </option>
              ))}
            </SelectField>
          ) : null}
        </div>
      ) : null}
      <p aria-live="polite" className="visually-hidden">
        {said}
      </p>
      <div
        aria-label={label}
        className={drag.drag ? 'aw-pboard is-dragging' : 'aw-pboard'}
        ref={scrollerRef}
        role="group"
        style={{ '--aw-board-columns': columns.length } as CSSProperties}
      >
        {columns.map((column) => (
          <Column
            board={props}
            column={column}
            held={held}
            key={column.key}
            moved={moved}
            onArrived={arrived}
            onCardKey={onCardKey}
            onCards={onCards}
            onMove={move}
            onOpen={(item) => {
              if (!drag.justDragged()) props.onOpen?.(item)
            }}
            onPointerDown={drag.onPointerDown}
            target={target === column.key}
          />
        ))}
      </div>
      {drag.drag && dragged ? (
        <div aria-hidden="true" className="aw-pboard__ghost" ref={ghostRef} style={{ width: drag.drag.width }}>
          <span className="aw-pboard__title">{props.title(dragged)}</span>
        </div>
      ) : null}
    </div>
  )
}

type ColumnProps<T extends { id: string }, S> = {
  board: BoardPieceProps<T, S>
  column: BoardColumn
  held: Held | null
  moved: readonly Moved<T>[]
  onArrived: (id: string) => void
  onCardKey: (event: ReactKeyboardEvent<HTMLElement>, item: T) => void
  onCards: (key: string, items: readonly T[]) => void
  onMove: (item: T, key: string) => void
  onOpen: (item: T) => void
  onPointerDown: (event: ReactPointerEvent<HTMLElement>, id: string) => void
  target: boolean
}

function Column<T extends { id: string }, S>(props: ColumnProps<T, S>) {
  const { Source, source } = props.board
  return (
    <Source column={props.column} source={source}>
      {(state) => <ColumnCards {...props} state={state} />}
    </Source>
  )
}

function ColumnCards<T extends { id: string }, S>({
  board,
  column,
  held,
  moved,
  onArrived,
  onCardKey,
  onCards,
  onMove,
  onOpen,
  onPointerDown,
  state,
  target,
}: ColumnProps<T, S> & { state: BoardColumnState<T> }) {
  const { columnOf } = board
  const loaded = state.items
  // A card stays in the column it's in now: one moved away leaves at once, one moved here joins.
  const cards = useMemo(() => {
    const away = new Set(moved.filter((entry) => entry.key !== column.key).map((entry) => entry.item.id))
    const here = loaded.filter((item) => !away.has(item.id) && columnOf(item) === column.key)
    const joined = moved.filter((entry) => entry.key === column.key && !here.some((item) => item.id === entry.item.id)).map((entry) => entry.item)
    return [...here, ...joined]
  }, [column.key, columnOf, loaded, moved])
  useEffect(() => onCards(column.key, cards), [cards, column.key, onCards])
  // Once this column's list has a card moved here, or has loaded again since
  // its save landed, the list shows what's true on its own.
  useEffect(() => {
    for (const entry of moved) {
      if (entry.key !== column.key) continue
      const listed = loaded.some((item) => item.id === entry.item.id && columnOf(item) === column.key)
      if (listed || (entry.savedAt !== undefined && state.fetchedFrom > entry.savedAt)) onArrived(entry.item.id)
    }
  }, [column.key, columnOf, loaded, moved, onArrived, state.fetchedFrom])
  const name = columnName(column)

  return (
    <section aria-label={name} className={target ? 'aw-pboard__column is-target' : 'aw-pboard__column'} data-column={column.key} data-drop-cell="">
      <header className="aw-pboard__head">
        {column.color ? <span aria-hidden className="aw-label__dot" style={{ backgroundColor: column.color }} /> : null}
        <span className="aw-pboard__label">{name}</span>
        <span className="aw-pboard__count">{column.count}</span>
      </header>
      {state.pending ? (
        <ListSkeleton label={`Loading ${name}`} rows={2} />
      ) : (
        <ol className="aw-pboard__cards">
          {cards.map((item) => (
            <Card board={board} held={held?.id === item.id} item={item} key={item.id} onCardKey={onCardKey} onMove={onMove} onOpen={onOpen} onPointerDown={onPointerDown} />
          ))}
        </ol>
      )}
      {state.failed ? (
        <Button onClick={state.onRetry} size="sm" variant="ghost">
          Try loading again
        </Button>
      ) : null}
      {state.hasMore ? (
        <Button busy={state.loadingMore} onClick={state.onLoadMore} size="sm" variant="ghost">
          Show more ({Math.max(0, column.count - loaded.length)})
        </Button>
      ) : null}
    </section>
  )
}

function Card<T extends { id: string }, S>({
  board,
  held,
  item,
  onCardKey,
  onMove,
  onOpen,
  onPointerDown,
}: {
  board: BoardPieceProps<T, S>
  held: boolean
  item: T
  onCardKey: (event: ReactKeyboardEvent<HTMLElement>, item: T) => void
  onMove: (item: T, key: string) => void
  onOpen: (item: T) => void
  onPointerDown: (event: ReactPointerEvent<HTMLElement>, id: string) => void
}) {
  const movable = Boolean(board.onMove)
  const name = board.title(item)
  const here = board.columnOf(item)
  const moveItems: MenuEntry[] = [
    { heading: board.movesBy ? `Move to (${board.movesBy})` : 'Move to' },
    ...board.columns.map((column) => ({ label: columnName(column), checked: column.key === here, onSelect: () => onMove(item, column.key) })),
  ]
  const className = ['aw-pboard__card', movable ? 'is-movable' : '', held ? 'is-held' : ''].filter(Boolean).join(' ')
  return (
    <li
      aria-current={board.openId === item.id || undefined}
      className={className}
      data-drag-id={item.id}
      onKeyDown={movable ? (event) => onCardKey(event, item) : undefined}
      onPointerDown={movable ? (event) => onPointerDown(event, item.id) : undefined}
    >
      <span className="aw-pboard__cardhead">
        <a
          aria-roledescription={movable ? 'movable card' : undefined}
          className="aw-pboard__title"
          href={board.href(item)}
          onClick={(event) => {
            if (event.metaKey || event.ctrlKey || event.shiftKey || !board.onOpen) return
            event.preventDefault()
            onOpen(item)
          }}
        >
          {name || 'Untitled'}
        </a>
        {board.badge ? <span className="aw-pboard__badge">{board.badge(item)}</span> : null}
        {movable ? <Menu items={moveItems} label={`Move ${name}`} placement="bottom-end" trigger={<IconButton className="aw-pboard__menu" icon="more" label={`Move ${name}`} size="sm" />} /> : null}
      </span>
      {board.details?.(item)}
    </li>
  )
}
