import { useQuery } from '@tanstack/react-query'
import { useEffect, useMemo, useRef, useState } from 'react'

import { opQuery } from '../api/ops'
import { Button, TextArea } from '../ui'
import { FieldValueView } from './FieldValueView'
import { normalizeValues, type FieldDefinition, type FormulaResult } from './fieldModel'

/** Where a formula is written: its collection, the fields it can read, and what the title is called there. */
export type FormulaContext = {
  collectionId: string
  fields: readonly FieldDefinition[]
  titleName: string
}

const RESULT_WORDS: Record<FormulaResult, string> = {
  number: 'a number',
  text: 'text',
  boolean: 'yes or no',
  date: 'a date',
  list: 'a list',
}

/** The `{partial` being typed before the cursor, if any. */
function openReference(text: string, cursor: number) {
  const before = text.slice(0, cursor)
  const open = before.lastIndexOf('{')
  if (open < 0 || before.lastIndexOf('}') > open || before.slice(open).includes('"')) return null
  return { start: open, partial: before.slice(open + 1).toLowerCase() }
}

function useDebounced<T>(value: T, delay: number) {
  const [settled, setSettled] = useState(value)
  useEffect(() => {
    const timer = window.setTimeout(() => setSettled(value), delay)
    return () => window.clearTimeout(timer)
  }, [value, delay])
  return settled
}

/**
 * A formula field's formula (T1.17): written with fields by name in braces,
 * with the fields offered as you type `{`, and checked and worked out on the
 * collection's first record as you type (`formula_preview`), so a mistake
 * shows before it's saved.
 */
export function FormulaEditor({
  context,
  fieldId,
  onChange,
  value,
}: {
  context: FormulaContext
  /** The formula field being edited; its own name isn't offered. */
  fieldId?: string
  onChange: (value: string) => void
  value: string
}) {
  const area = useRef<HTMLTextAreaElement>(null)
  const [cursor, setCursor] = useState(value.length)
  const names = useMemo(
    () => [context.titleName, ...context.fields.filter((field) => !field.archived && field.id !== fieldId).map((field) => field.name)],
    [context, fieldId],
  )
  const typing = openReference(value, cursor)
  const offered = typing ? names.filter((name) => name.toLowerCase().includes(typing.partial)).slice(0, 8) : names.slice(0, 8)
  const insert = (name: string) => {
    const start = typing ? typing.start : cursor
    const next = `${value.slice(0, start)}{${name}}${value.slice(cursor)}`
    const at = start + name.length + 2
    onChange(next)
    setCursor(at)
    window.requestAnimationFrame(() => {
      area.current?.focus()
      area.current?.setSelectionRange(at, at)
    })
  }
  const formula = useDebounced(value.trim(), 400)
  const preview = useQuery({
    ...opQuery('formula_preview', { collection: context.collectionId, formula, ...(fieldId ? { field: fieldId } : {}) }),
    enabled: formula.length > 0,
    staleTime: 10_000,
  })
  const answer = preview.data
  const previewField: FieldDefinition | null = answer?.result
    ? {
        id: 'preview',
        name: 'Preview',
        description: '',
        type: 'formula',
        options: [],
        target: null,
        formula: { source: formula, result: answer.result },
        position: 1,
        archived: false,
        version: 1,
      }
    : null
  const previewValue = normalizeValues({ value: answer?.value ?? null }).value
  return (
    <div className="aw-formula">
      <TextArea
        className="aw-formula__source"
        hint={'Name fields in braces and join text with &, such as {Price} * {Quantity} or IF({Due} < TODAY(), "Late", "").'}
        label="Formula"
        maxLength={2000}
        onChange={(event) => {
          onChange(event.target.value)
          setCursor(event.target.selectionStart ?? event.target.value.length)
        }}
        onKeyUp={(event) => setCursor(event.currentTarget.selectionStart ?? value.length)}
        onClick={(event) => setCursor(event.currentTarget.selectionStart ?? value.length)}
        ref={area}
        required
        rows={3}
        spellCheck={false}
        value={value}
      />
      {offered.length ? (
        <div aria-label={typing ? 'Fields matching what you typed' : 'Fields'} className="aw-formula__fields" role="group">
          {offered.map((name) => (
            <Button key={name} onClick={() => insert(name)} size="sm" variant="ghost">
              {`{${name}}`}
            </Button>
          ))}
        </div>
      ) : null}
      <p aria-live="polite" className="aw-formula__preview">
        {!formula ? null : preview.isError ? (
          'Couldn’t check the formula just now.'
        ) : !answer ? (
          'Checking…'
        ) : answer.problem ? (
          <span className="aw-prop__formula-error">{answer.problem}</span>
        ) : (
          <>
            Gives {RESULT_WORDS[answer.result ?? 'text']}.{' '}
            {answer.record && previewField ? (
              <>
                On #{answer.record.number} {answer.record.title || 'Untitled'}:{' '}
                <FieldValueView empty="nothing" field={previewField} value={previewValue} />
              </>
            ) : (
              'No records to try it on yet.'
            )}
          </>
        )}
      </p>
    </div>
  )
}
