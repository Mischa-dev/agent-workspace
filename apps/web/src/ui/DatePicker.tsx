import { useId, useRef, useState, type KeyboardEvent } from 'react'
import { CalendarDays, ChevronLeft } from 'lucide-react'

import { Icon } from './icons'
import { Popover } from './Popover'
import { addDays, addMonths, formatDateLabel, fromIsoDate, monthGrid, toIsoDate } from './dateModel'

const WEEKDAYS = ['Mo', 'Tu', 'We', 'Th', 'Fr', 'Sa', 'Su']
const WEEKDAY_NAMES = ['Monday', 'Tuesday', 'Wednesday', 'Thursday', 'Friday', 'Saturday', 'Sunday']

/** A month calendar with arrow-key movement. Enter or a click picks a day. */
export function Calendar({ label, onSelect, value }: { label: string; onSelect: (value: string) => void; value: string | null }) {
  const selected = fromIsoDate(value)
  const [focused, setFocused] = useState(() => selected ?? new Date())
  const grid = useRef<HTMLDivElement>(null)
  const headingId = useId()
  const today = toIsoDate(new Date())
  const days = monthGrid(focused)

  const focusDay = (date: Date) => {
    setFocused(date)
    window.requestAnimationFrame(() => grid.current?.querySelector<HTMLElement>(`[data-date="${toIsoDate(date)}"]`)?.focus())
  }
  const onKeyDown = (event: KeyboardEvent<HTMLDivElement>) => {
    const moves: Record<string, () => Date> = {
      ArrowLeft: () => addDays(focused, -1),
      ArrowRight: () => addDays(focused, 1),
      ArrowUp: () => addDays(focused, -7),
      ArrowDown: () => addDays(focused, 7),
      PageUp: () => addMonths(focused, event.shiftKey ? -12 : -1),
      PageDown: () => addMonths(focused, event.shiftKey ? 12 : 1),
      Home: () => addDays(focused, -((focused.getDay() + 6) % 7)),
      End: () => addDays(focused, 6 - ((focused.getDay() + 6) % 7)),
    }
    const move = moves[event.key]
    if (!move) return
    event.preventDefault()
    focusDay(move())
  }

  return (
    <div className="aw-calendar">
      <div className="aw-calendar__head">
        <button aria-label="Previous month" className="aw-iconbtn aw-iconbtn--sm" onClick={() => setFocused(addMonths(focused, -1))} type="button">
          <Icon name={ChevronLeft} size={16} />
        </button>
        <h3 aria-live="polite" className="aw-calendar__month" id={headingId}>
          {focused.toLocaleDateString([], { month: 'long', year: 'numeric' })}
        </h3>
        <button aria-label="Next month" className="aw-iconbtn aw-iconbtn--sm" onClick={() => setFocused(addMonths(focused, 1))} type="button">
          <Icon name="chevron-right" size={16} />
        </button>
      </div>
      <div aria-label={label} aria-labelledby={headingId} className="aw-calendar__grid" onKeyDown={onKeyDown} ref={grid} role="grid">
        <div className="aw-calendar__row" role="row">
          {WEEKDAYS.map((day, index) => (
            <span aria-label={WEEKDAY_NAMES[index]} className="aw-calendar__weekday" key={day} role="columnheader">
              {day}
            </span>
          ))}
        </div>
        {Array.from({ length: 6 }, (_, week) => (
          <div className="aw-calendar__row" key={week} role="row">
            {days.slice(week * 7, week * 7 + 7).map((day) => {
              const iso = toIsoDate(day)
              const isFocused = iso === toIsoDate(focused)
              const outside = day.getMonth() !== focused.getMonth()
              return (
                <span key={iso} role="gridcell" aria-selected={iso === value}>
                  <button
                    aria-current={iso === today ? 'date' : undefined}
                    aria-label={day.toLocaleDateString([], { weekday: 'long', month: 'long', day: 'numeric', year: 'numeric' })}
                    className={['aw-calendar__day', outside ? 'is-outside' : '', iso === value ? 'is-selected' : '', iso === today ? 'is-today' : '']
                      .filter(Boolean)
                      .join(' ')}
                    data-date={iso}
                    onClick={() => onSelect(iso)}
                    tabIndex={isFocused ? 0 : -1}
                    type="button"
                  >
                    {day.getDate()}
                  </button>
                </span>
              )
            })}
          </div>
        ))}
      </div>
    </div>
  )
}

/**
 * A date field: a button showing the date that opens a calendar, with a typed
 * field for people who'd rather type, and Clear. `value` is `YYYY-MM-DD` or null.
 */
export function DatePicker({
  className,
  disabled,
  label,
  onChange,
  placeholder = 'No date',
  prefix,
  value,
}: {
  className?: string
  disabled?: boolean
  label: string
  onChange: (value: string | null) => void
  placeholder?: string
  /** A word before a set date, such as "Due", where the label isn't next to it. */
  prefix?: string
  value: string | null
}) {
  const [anchor, setAnchor] = useState<HTMLButtonElement | null>(null)
  const [typed, setTyped] = useState(value ?? '')
  const close = () => setAnchor(null)
  const pick = (next: string | null) => {
    onChange(next)
    close()
  }
  return (
    <>
      <button
        aria-expanded={anchor !== null}
        aria-haspopup="dialog"
        aria-label={`${label}: ${value ? formatDateLabel(value) : placeholder}`}
        className={['aw-datepicker', value ? '' : 'is-empty', className].filter(Boolean).join(' ')}
        disabled={disabled}
        onClick={(event) => {
          setTyped(value ?? '')
          setAnchor(anchor ? null : event.currentTarget)
        }}
        type="button"
      >
        <Icon name={CalendarDays} size={14} />
        <span>{value ? `${prefix ? `${prefix} ` : ''}${formatDateLabel(value)}` : placeholder}</span>
      </button>
      <Popover anchor={anchor} className="aw-datepicker__panel" label={label} onClose={close} open={anchor !== null}>
        <form
          className="aw-datepicker__typed"
          onSubmit={(event) => {
            event.preventDefault()
            if (!typed) pick(null)
            else if (fromIsoDate(typed)) pick(typed)
          }}
        >
          <input
            aria-label={`${label}, typed as year-month-day`}
            className="aw-datepicker__input"
            onChange={(event) => setTyped(event.target.value)}
            type="date"
            value={typed}
          />
          <button className="aw-btn aw-btn--secondary aw-btn--sm" type="submit">
            <span className="aw-btn__label">Set</span>
          </button>
        </form>
        <Calendar label={label} onSelect={pick} value={value} />
        <div className="aw-datepicker__foot">
          <button className="aw-btn aw-btn--ghost aw-btn--sm" onClick={() => pick(toIsoDate(new Date()))} type="button">
            <span className="aw-btn__label">Today</span>
          </button>
          {value ? (
            <button className="aw-btn aw-btn--ghost aw-btn--sm" onClick={() => pick(null)} type="button">
              <span className="aw-btn__label">Clear</span>
            </button>
          ) : null}
        </div>
      </Popover>
    </>
  )
}
