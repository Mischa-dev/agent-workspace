import { ChevronLeft } from 'lucide-react'
import { useMemo, useRef, type ReactNode } from 'react'

import { Button, EmptyState, IconButton, ListSkeleton } from '../ui'
import { addMonths, monthGrid, toIsoDate } from '../ui/dateModel'
import { useBoardDrag } from '../ui/useBoardDrag'
import { usePhoneWidth } from '../ui/useCoarsePointer'
import './calendar.css'

// The calendar piece (W1.20): a month, each item on the day it falls on. A
// day shows its first few items and "N more"; its + adds one on that day;
// dragging an item to another day moves it there. On a phone the month is a
// list of the days that have items. Which month, what's on it and how an
// item moves are the owner's: collections today. (Timelines are T1.14's,
// on the same grid and drag primitives.)

/** Items a day shows before "N more". */
const DAY_LIMIT = 3

const WEEKDAYS = Array.from({ length: 7 }, (_, index) => new Date(2024, 0, 1 + index).toLocaleDateString([], { weekday: 'short' }))

export type CalendarPieceProps<T extends { id: string }> = {
  /** Any day in the month shown. */
  month: Date
  /** Another month, or null for this one. */
  onMonth: (month: Date | null) => void
  items: readonly T[]
  /** The day an item is on, as `YYYY-MM-DD`, or null for none. */
  dayOf: (item: T) => string | null
  title: (item: T) => string
  href: (item: T) => string
  onOpen?: (item: T) => void
  openId?: string | null
  /** Moves an item to another day; without it, items stay put. */
  onMoveDay?: (item: T, day: string) => void
  /** Adds an item on a day; without it, days have no +. */
  onAdd?: (day: string) => void
  /** What an item is called, for the + ("New record on 3 November"). */
  noun: string
  /** Beside the month's name: a count, or "Loading…". */
  summary?: ReactNode
  /** Above the grid, such as a warning that not all of the month loaded. */
  notice?: ReactNode
  pending?: boolean
  /** Nothing loaded, and the load failed. */
  failed?: boolean
  onRetry?: () => void
}

/** Items by the day they fall on, in the order they came. */
function itemsByDay<T extends { id: string }>(items: readonly T[], dayOf: (item: T) => string | null) {
  const days = new Map<string, T[]>()
  for (const item of items) {
    const day = dayOf(item)
    if (day) days.set(day, [...(days.get(day) ?? []), item])
  }
  return days
}

export function CalendarPiece<T extends { id: string }>(props: CalendarPieceProps<T>) {
  const { dayOf, items, month, onAdd, onMonth, onMoveDay, noun } = props
  const phone = usePhoneWidth()
  const days = useMemo(() => monthGrid(month), [month])
  const today = toIsoDate(new Date())
  const byDay = useMemo(() => itemsByDay(items, dayOf), [dayOf, items])
  const ghostRef = useRef<HTMLDivElement>(null)
  const scrollerRef = useRef<HTMLDivElement>(null)
  const drag = useBoardDrag({
    enabled: Boolean(onMoveDay) && !phone,
    ghostRef,
    scrollerRef,
    onDrop: (id, target) => {
      const item = items.find((entry) => entry.id === id)
      if (item && dayOf(item) !== target.columnKey) onMoveDay?.(item, target.columnKey)
    },
  })
  const dragged = drag.drag ? items.find((entry) => entry.id === drag.drag?.cardId) : undefined
  const title = month.toLocaleDateString([], { month: 'long', year: 'numeric' })
  const inMonth = (day: Date) => day.getMonth() === month.getMonth()
  const addLabel = (day: Date) => `New ${noun} on ${day.toLocaleDateString([], { month: 'long', day: 'numeric' })}`

  const itemLink = (item: T): ReactNode => (
    <a
      aria-current={item.id === props.openId || undefined}
      className="aw-ccal__record"
      data-drag-id={item.id}
      href={props.href(item)}
      key={item.id}
      onClick={(event) => {
        if (event.metaKey || event.ctrlKey || event.shiftKey || !props.onOpen) return
        event.preventDefault()
        if (!drag.justDragged()) props.onOpen(item)
      }}
      onPointerDown={onMoveDay && !phone ? (event) => drag.onPointerDown(event, item.id) : undefined}
    >
      {props.title(item)}
    </a>
  )

  return (
    <div className="aw-ccal">
      <div className="aw-ccal__bar">
        <IconButton icon={ChevronLeft} label="Last month" onClick={() => onMonth(addMonths(month, -1))} size="sm" />
        <h2 className="aw-ccal__month">{title}</h2>
        <IconButton icon="chevron-right" label="Next month" onClick={() => onMonth(addMonths(month, 1))} size="sm" />
        <Button onClick={() => onMonth(null)} size="sm" variant="ghost">
          Today
        </Button>
        {props.summary ? (
          <span aria-live="polite" className="aw-records__summary">
            {props.summary}
          </span>
        ) : null}
      </div>
      {props.notice}
      {props.pending ? (
        <ListSkeleton label="Loading the month" rows={5} />
      ) : props.failed ? (
        <EmptyState action={props.onRetry ? <Button onClick={props.onRetry}>Try again</Button> : null} icon="circle-alert" title="The month didn’t load">
          Check the connection and try again.
        </EmptyState>
      ) : phone ? (
        <Agenda addLabel={addLabel} byDay={byDay} days={days.filter(inMonth)} itemLink={itemLink} noun={noun} onAdd={onAdd} today={today} />
      ) : (
        <div className={drag.drag ? 'aw-ccal__grid is-dragging' : 'aw-ccal__grid'} ref={scrollerRef}>
          <div aria-hidden="true" className="aw-ccal__weekdays">
            {WEEKDAYS.map((name) => (
              <span className="aw-ccal__weekday" key={name}>
                {name}
              </span>
            ))}
          </div>
          {Array.from({ length: days.length / 7 }, (_, week) => (
            <div className="aw-ccal__week" key={week}>
              {days.slice(week * 7, week * 7 + 7).map((day) => {
                const iso = toIsoDate(day)
                const entries = byDay.get(iso) ?? []
                const target = drag.drag?.target?.columnKey === iso
                return (
                  <div
                    aria-label={day.toLocaleDateString([], { weekday: 'long', month: 'long', day: 'numeric' })}
                    className={['aw-ccal__day', inMonth(day) ? '' : 'is-outside', iso === today ? 'is-today' : '', target ? 'is-target' : ''].filter(Boolean).join(' ')}
                    data-column={iso}
                    data-drop-cell=""
                    key={iso}
                    role="group"
                  >
                    <span className="aw-ccal__dayhead">
                      <span className="aw-ccal__date">{day.getDate()}</span>
                      {onAdd ? <IconButton className="aw-ccal__add" icon="plus" label={addLabel(day)} onClick={() => onAdd(iso)} size="sm" /> : null}
                    </span>
                    {entries.slice(0, entries.length > DAY_LIMIT + 1 ? DAY_LIMIT : DAY_LIMIT + 1).map(itemLink)}
                    {entries.length > DAY_LIMIT + 1 ? (
                      <details className="aw-ccal__more">
                        <summary>{entries.length - DAY_LIMIT} more</summary>
                        {entries.slice(DAY_LIMIT).map(itemLink)}
                      </details>
                    ) : null}
                  </div>
                )
              })}
            </div>
          ))}
        </div>
      )}
      {drag.drag && dragged ? (
        <div aria-hidden="true" className="aw-ccal__ghost" ref={ghostRef} style={{ width: drag.drag.width }}>
          {props.title(dragged)}
        </div>
      ) : null}
    </div>
  )
}

/** On a phone: the month's days that have items, and today. */
function Agenda<T extends { id: string }>({
  addLabel,
  byDay,
  days,
  itemLink,
  noun,
  onAdd,
  today,
}: {
  addLabel: (day: Date) => string
  byDay: Map<string, T[]>
  days: readonly Date[]
  itemLink: (item: T) => ReactNode
  noun: string
  onAdd?: (day: string) => void
  today: string
}) {
  const shown = days.filter((day) => byDay.has(toIsoDate(day)) || toIsoDate(day) === today)
  if (!shown.length) return <p className="aw-ccal__none">No {noun}s this month.</p>
  return (
    <ol className="aw-ccal__agenda">
      {shown.map((day) => {
        const iso = toIsoDate(day)
        return (
          <li className={iso === today ? 'aw-ccal__agendaday is-today' : 'aw-ccal__agendaday'} key={iso}>
            <span className="aw-ccal__dayhead">
              <span className="aw-ccal__agendadate">{day.toLocaleDateString([], { weekday: 'short', month: 'short', day: 'numeric' })}</span>
              {onAdd ? <IconButton icon="plus" label={addLabel(day)} onClick={() => onAdd(iso)} size="sm" /> : null}
            </span>
            {(byDay.get(iso) ?? []).map(itemLink)}
          </li>
        )
      })}
    </ol>
  )
}
