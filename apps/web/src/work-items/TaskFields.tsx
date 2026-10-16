import { useLayoutEffect, useRef, useState, type KeyboardEvent } from 'react'

import { Button, TextArea } from '../ui'
import { formatEstimate } from './historyFormat'
import type { EstimateUnit } from './types'

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

export function TitleField({ disabled, onSave, value }: { disabled: boolean; onSave: (title: string) => void; value: string }) {
  const { draft, setDraft, setEditing } = useDraft(value)
  const ref = useRef<HTMLTextAreaElement>(null)
  useLayoutEffect(() => {
    const element = ref.current
    if (!element) return
    element.style.height = 'auto'
    element.style.height = `${element.scrollHeight}px`
  }, [draft])

  const commit = () => {
    setEditing(false)
    const title = draft.trim()
    if (!title) setDraft(value)
    else if (title !== value) onSave(title)
  }
  const onKeyDown = (event: KeyboardEvent<HTMLTextAreaElement>) => {
    if (event.nativeEvent.isComposing) return
    if (event.key === 'Enter') {
      event.preventDefault()
      event.currentTarget.blur()
    } else if (event.key === 'Escape') {
      event.preventDefault()
      event.stopPropagation()
      setDraft(value)
      setEditing(false)
      window.requestAnimationFrame(() => ref.current?.blur())
    }
  }
  if (disabled) return <h1 className="aw-workitem__heading">{value}</h1>
  return (
    <textarea
      aria-label="Title"
      className="aw-workitem__title"
      maxLength={240}
      onBlur={commit}
      onChange={(event) => setDraft(event.target.value)}
      onFocus={() => setEditing(true)}
      onKeyDown={onKeyDown}
      placeholder="Work item title"
      ref={ref}
      rows={1}
      value={draft}
    />
  )
}

/** The description as plain text with line breaks. Ctrl or Cmd + Enter saves, Escape puts it back. */
export function DescriptionField({ disabled, onSave, value }: { disabled: boolean; onSave: (description: string) => void; value: string }) {
  const { draft, setDraft, editing, setEditing } = useDraft(value)
  const ref = useRef<HTMLTextAreaElement>(null)
  if (disabled) {
    return <p className={value ? 'aw-workitem__text' : 'aw-workitem__text is-empty'}>{value || 'No description.'}</p>
  }
  const commit = () => {
    setEditing(false)
    if (draft !== value) onSave(draft)
  }
  return (
    <div className="aw-taskdesc">
      <TextArea
        aria-label="Description"
        className="aw-workitem__desc"
        maxLength={100000}
        onBlur={(event) => {
          // Leaving for the Cancel button shouldn't save first.
          if ((event.relatedTarget as HTMLElement | null)?.dataset.cancel === 'description') return
          commit()
        }}
        onChange={(event) => setDraft(event.target.value)}
        onFocus={() => setEditing(true)}
        onKeyDown={(event) => {
          if (event.key === 'Enter' && (event.metaKey || event.ctrlKey)) {
            event.preventDefault()
            event.currentTarget.blur()
          } else if (event.key === 'Escape') {
            event.preventDefault()
            event.stopPropagation()
            setDraft(value)
            setEditing(false)
            window.requestAnimationFrame(() => ref.current?.blur())
          }
        }}
        placeholder="Add a description: the outcome you want, any limits, and useful context."
        ref={ref}
        rows={5}
        value={draft}
      />
      {editing ? (
        <div className="aw-taskdesc__actions">
          <span className="aw-taskdesc__hint">Saves when you click away · Ctrl+Enter</span>
          <Button
            data-cancel="description"
            onClick={() => {
              setDraft(value)
              setEditing(false)
            }}
            size="sm"
            variant="ghost"
          >
            Cancel
          </Button>
        </div>
      ) : null}
    </div>
  )
}

export function EstimateField({
  disabled,
  onSave,
  unit,
  value,
}: {
  disabled: boolean
  onSave: (estimate: number | null) => void
  unit: EstimateUnit
  value: number | null
}) {
  const text = value === null ? '' : String(value)
  const { draft, setDraft, setEditing } = useDraft(text)
  const [invalid, setInvalid] = useState(false)
  if (disabled) return <span className="aw-prop__value">{value === null ? 'No estimate' : formatEstimate(value, unit)}</span>
  const commit = () => {
    setEditing(false)
    const trimmed = draft.trim()
    if (trimmed === text) return
    if (!trimmed) return onSave(null)
    const parsed = Number(trimmed)
    if (!Number.isFinite(parsed) || parsed < 0 || parsed > 100000) {
      setInvalid(true)
      setDraft(text)
      return
    }
    setInvalid(false)
    onSave(Math.round(parsed * 10) / 10)
  }
  return (
    <span className="aw-estimate">
      <input
        aria-describedby={invalid ? 'estimate-note' : undefined}
        aria-invalid={invalid || undefined}
        aria-label={`Estimate in ${unit}`}
        className="aw-estimate__input"
        inputMode="decimal"
        onBlur={commit}
        onChange={(event) => setDraft(event.target.value)}
        onFocus={() => setEditing(true)}
        onKeyDown={(event) => {
          if (event.key === 'Enter') event.currentTarget.blur()
        }}
        placeholder="None"
        value={draft}
      />
      <span className="aw-estimate__unit">{unit}</span>
      {invalid ? (
        <span className="aw-estimate__note" id="estimate-note" role="alert">
          Use a number from 0 to 100,000.
        </span>
      ) : null}
    </span>
  )
}
