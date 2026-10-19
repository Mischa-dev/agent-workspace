import { useLayoutEffect, useRef, useState, type KeyboardEvent } from 'react'

import { Button } from '../ui'
import { Popover } from '../ui/pickers'
import { formatEstimate } from './historyFormat'
import { TaskGlyph } from './TaskGlyph'
import type { EstimateUnit, WorkItem } from './types'
import type { TaskSave } from './useTaskSave'

/**
 * Text that edits in place and saves when you leave it. While you type, a
 * newer value from the server doesn't overwrite your words; once you're done
 * the field shows whatever was saved.
 */
function useDraft(value: string) {
  const [draft, setDraft] = useState(value)
  const [editing, setEditing] = useState(false)
  const [shown, setShown] = useState(value)
  if (!editing && shown !== value) {
    setShown(value)
    setDraft(value)
  }
  return { draft, setDraft, editing, setEditing }
}

/** The title, edited in place. Enter saves, Escape puts it back. Typed text survives a clash with someone else's edit. */
export function TitleField({ disabled, item, save }: { disabled: boolean; item: WorkItem; save: TaskSave }) {
  const pending = save.pending('title')
  const value = pending?.text ?? item.title
  const { draft, setDraft, setEditing } = useDraft(value)
  const ref = useRef<HTMLTextAreaElement>(null)
  const base = useRef('')
  const before = useRef('')
  useLayoutEffect(() => {
    const element = ref.current
    if (!element) return
    element.style.height = 'auto'
    element.style.height = `${element.scrollHeight}px`
  }, [draft])

  const commit = () => {
    setEditing(false)
    const title = draft.trim()
    if (!title) return setDraft(value)
    if (title === before.current.trim()) return
    // Editing again after a clash or a failed save means "this one".
    if (pending?.state === 'conflict') save.keepMine('title', { title })
    else if (pending?.state === 'failed') save.retry('title', { title })
    else save.save('title', { title }, base.current)
  }
  const onKeyDown = (event: KeyboardEvent<HTMLTextAreaElement>) => {
    if (event.nativeEvent.isComposing) return
    if (event.key === 'Enter') {
      event.preventDefault()
      event.currentTarget.blur()
    } else if (event.key === 'Escape') {
      event.preventDefault()
      event.stopPropagation()
      setDraft(before.current)
      setEditing(false)
      window.requestAnimationFrame(() => ref.current?.blur())
    }
  }
  if (disabled) return <p className="aw-task__heading">{value}</p>
  return (
    <textarea
      aria-label="Title"
      className="aw-task__title"
      maxLength={240}
      onBlur={commit}
      onChange={(event) => setDraft(event.target.value)}
      onFocus={() => {
        base.current = save.baseFor('title')
        before.current = draft
        setEditing(true)
      }}
      onKeyDown={onKeyDown}
      placeholder="Task title"
      ref={ref}
      rows={1}
      value={draft}
    />
  )
}

const SCALES: Record<EstimateUnit, number[]> = { points: [1, 2, 3, 5, 8, 13], hours: [1, 2, 4, 8, 16, 24] }

/** The estimate: a button that opens a small picker with the usual sizes and a field for any other number. */
export function EstimateField({
  className,
  disabled,
  onSave,
  placeholder,
  unit,
  value,
}: {
  className: string
  disabled: boolean
  onSave: (estimate: number | null) => void
  placeholder: string
  unit: EstimateUnit
  value: number | null
}) {
  const [anchor, setAnchor] = useState<HTMLElement | null>(null)
  const [typed, setTyped] = useState('')
  const [invalid, setInvalid] = useState(false)
  const shown = value === null ? placeholder : formatEstimate(value, unit)
  const content = (
    <>
      <TaskGlyph name="estimate" />
      <span className={value === null ? 'aw-prop__empty' : undefined}>{shown}</span>
    </>
  )
  if (disabled) return <span className={`${className} is-static`}>{content}</span>
  const pick = (next: number | null) => {
    setAnchor(null)
    if (next !== value) onSave(next)
  }
  return (
    <>
      <button
        aria-expanded={anchor !== null}
        aria-haspopup="dialog"
        aria-label={`Estimate: ${value === null ? 'none' : formatEstimate(value, unit)}. Change`}
        className={className}
        onClick={(event) => {
          setTyped(value === null ? '' : String(value))
          setInvalid(false)
          setAnchor(anchor ? null : event.currentTarget)
        }}
        type="button"
      >
        {content}
      </button>
      <Popover anchor={anchor} className="aw-estimate" label="Estimate" onClose={() => setAnchor(null)} open={anchor !== null} width={260}>
        <form
          className="aw-estimate__form"
          onSubmit={(event) => {
            event.preventDefault()
            const trimmed = typed.trim()
            if (!trimmed) return pick(null)
            const parsed = Number(trimmed)
            if (!Number.isFinite(parsed) || parsed < 0 || parsed > 100000) return setInvalid(true)
            pick(Math.round(parsed * 10) / 10)
          }}
        >
          <div className="aw-estimate__scale" role="group" aria-label={`Common sizes in ${unit}`}>
            {SCALES[unit].map((size) => (
              <button aria-pressed={value === size} className="aw-estimate__size" key={size} onClick={() => pick(size)} type="button">
                {size}
              </button>
            ))}
          </div>
          <div className="aw-estimate__row">
            <input
              aria-describedby={invalid ? 'estimate-note' : undefined}
              aria-invalid={invalid || undefined}
              aria-label={`Estimate in ${unit}`}
              className="aw-estimate__input"
              inputMode="decimal"
              onChange={(event) => setTyped(event.target.value)}
              placeholder="Other"
              value={typed}
            />
            <span className="aw-estimate__unit">{unit}</span>
            <Button size="sm" type="submit">
              Set
            </Button>
          </div>
          {invalid ? (
            <p className="aw-estimate__note" id="estimate-note" role="alert">
              Use a number from 0 to 100,000.
            </p>
          ) : null}
          {value !== null ? (
            <Button className="aw-estimate__clear" onClick={() => pick(null)} size="sm" variant="ghost">
              Clear estimate
            </Button>
          ) : null}
        </form>
      </Popover>
    </>
  )
}
