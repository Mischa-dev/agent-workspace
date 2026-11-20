// Find and replace, and saved filter views, for the sheet page (C1.16).
import { useEffect, useMemo, useRef, useState } from 'react'
import { useQuery, useQueryClient } from '@tanstack/react-query'

import { callOp, opPrefix, opQuery, type OpInput } from '../api/ops'
import { Banner, Button, Checkbox, Dialog, IconButton, SelectField, TextField, describeError } from '../ui'
import type { ConditionalRule, SheetChange, SheetTab } from './api'
import { columnName, columnNumber, parseRange } from './sheetModel'
import { replaced, type SheetView } from './sheetViews'

/**
 * The find bar: matches in reading order across the sheet's tabs, Enter
 * for the next one, and (for editors) replacing them all, each only if the
 * cell hasn't changed since it was found.
 */
export function SheetFind({
  editable,
  onClose,
  onGo,
  onReplace,
  sheetId,
  tabs,
}: {
  editable: boolean
  onClose: () => void
  onGo: (tabId: string, row: number, column: number) => void
  onReplace: (changes: SheetChange[]) => Promise<boolean>
  sheetId: string
  tabs: SheetTab[]
}) {
  const [query, setQuery] = useState('')
  const [settled, setSettled] = useState('')
  const [replacement, setReplacement] = useState('')
  const [matchCase, setMatchCase] = useState(false)
  const [wholeCell, setWholeCell] = useState(false)
  const [inFormulas, setInFormulas] = useState(false)
  // Which match is current, for the answer it was picked from.
  const [cursor, setCursor] = useState<{ of: unknown; index: number }>({ of: null, index: 0 })
  const [busy, setBusy] = useState(false)
  const field = useRef<HTMLInputElement>(null)
  useEffect(() => field.current?.focus(), [])
  useEffect(() => {
    const timer = window.setTimeout(() => setSettled(query), 250)
    return () => window.clearTimeout(timer)
  }, [query])
  const input = { sheetId, query: settled, matchCase, wholeCell, inFormulas }
  const search = useQuery({ ...opQuery('sheet_search', input), enabled: settled.trim().length > 0 })
  const matches = useMemo(() => search.data?.matches ?? [], [search.data])
  const at = cursor.of === search.data ? cursor.index : 0
  const setAt = (index: number) => setCursor({ of: search.data, index })

  const go = (index: number) => {
    if (!matches.length) return
    const next = (index + matches.length) % matches.length
    setAt(next)
    const match = matches[next]
    const tab = tabs.find((entry) => entry.name === match.tab)
    const range = parseRange(match.cell)
    if (tab && range) onGo(tab.id, range.top, range.left)
  }

  const replaceAll = async () => {
    if (!search.data) return
    const changes: SheetChange[] = []
    for (const match of matches) {
      const tab = tabs.find((entry) => entry.name === match.tab)
      const range = parseRange(match.cell)
      // A formula changes only when looking in formulas; a value worked out by one stays.
      if (!tab || !range || (match.input.startsWith('=') && !inFormulas)) continue
      const next = replaced(match.input, settled, replacement, { matchCase, wholeCell })
      if (next !== match.input) changes.push({ type: 'cell', tab: tab.id, row: range.top, column: range.left, input: next, ifRevision: search.data.revision })
    }
    if (!changes.length) return
    setBusy(true)
    await onReplace(changes)
    setBusy(false)
    void search.refetch()
  }

  const count = search.data ? `${matches.length ? at + 1 : 0} of ${matches.length}${search.data.truncated ? '+' : ''}` : ''
  return (
    <div className="aw-sheet__find" role="search">
      <TextField
        aria-label="Find in sheet"
        compact
        onChange={(event) => setQuery(event.target.value)}
        onKeyDown={(event) => {
          if (event.key === 'Enter') {
            event.preventDefault()
            go(event.shiftKey ? at - 1 : at + (matches.length && search.data ? 1 : 0))
          } else if (event.key === 'Escape') onClose()
        }}
        placeholder="Find"
        ref={field}
        value={query}
      />
      <span aria-live="polite" className="aw-sheet__find-count">
        {count}
      </span>
      <IconButton disabled={!matches.length} icon="arrow-up" label="Previous match" onClick={() => go(at - 1)} size="sm" />
      <IconButton disabled={!matches.length} icon="arrow-down" label="Next match" onClick={() => go(at + 1)} size="sm" />
      <Checkbox checked={matchCase} label="Match case" onChange={setMatchCase} />
      <Checkbox checked={wholeCell} label="Whole cell" onChange={setWholeCell} />
      <Checkbox checked={inFormulas} label="In formulas" onChange={setInFormulas} />
      {editable ? (
        <>
          <TextField aria-label="Replace with" compact onChange={(event) => setReplacement(event.target.value)} placeholder="Replace with" value={replacement} />
          <Button busy={busy} disabled={!matches.length} onClick={() => void replaceAll()} size="sm" variant="ghost">
            Replace all
          </Button>
        </>
      ) : null}
      <IconButton icon="x" label="Close find" onClick={onClose} size="sm" />
    </div>
  )
}

const OPS: [string, string][] = [
  ['equals', 'is'],
  ['notEquals', 'is not'],
  ['contains', 'contains'],
  ['notContains', 'doesn’t contain'],
  ['greaterThan', 'is more than'],
  ['lessThan', 'is less than'],
  ['atLeast', 'is at least'],
  ['atMost', 'is at most'],
  ['empty', 'is empty'],
  ['notEmpty', 'isn’t empty'],
]
const NO_VALUE = new Set(['empty', 'notEmpty'])

type FilterDraft = { column: string; op: string; value: string }

/** Makes a saved view, or changes one: filters, an order and hidden columns. */
export function SheetViewDialog({
  canShare,
  columns,
  onClose,
  onSaved,
  sheetId,
  tabId,
  view,
}: {
  canShare: boolean
  /** How many columns the tab uses, for the column choices. */
  columns: number
  onClose: () => void
  onSaved: (view: SheetView) => void
  sheetId: string
  tabId: string
  view: SheetView | null
}) {
  const client = useQueryClient()
  const [name, setName] = useState(view?.name ?? '')
  const [shared, setShared] = useState(view?.shared ?? false)
  const [filters, setFilters] = useState<FilterDraft[]>(
    view?.filters.filter((filter) => filter.op !== 'oneOf').map((filter) => ({ column: filter.column, op: filter.op, value: filter.value == null ? '' : String(filter.value) })) ?? [],
  )
  // Pick-from-a-list filters (made from a column's menu) aren't edited here; saving keeps them.
  const listFilters = view?.filters.filter((filter) => filter.op === 'oneOf') ?? []
  const [sortColumn, setSortColumn] = useState(view?.sort[0]?.column ?? '')
  const [descending, setDescending] = useState(view?.sort[0]?.descending ?? false)
  const [hidden, setHidden] = useState(view?.hiddenColumns.join(', ') ?? '')
  const [busy, setBusy] = useState(false)
  const [problem, setProblem] = useState<string | null>(null)
  const letters = Array.from({ length: Math.max(1, Math.min(columns, 200)) }, (_, index) => columnName(index + 1))

  const submit = async () => {
    setProblem(null)
    if (!name.trim()) return setProblem('Give the view a name.')
    const hiddenColumns = hidden
      .split(/[\s,]+/)
      .map((letter) => letter.trim().toUpperCase())
      .filter(Boolean)
    if (hiddenColumns.some((letter) => !columnNumber(letter))) return setProblem('Name hidden columns by their letters, such as C, E.')
    const body: Required<Pick<OpInput<'sheet_view_create'>, 'name' | 'filters' | 'sort' | 'hiddenColumns'>> = {
      name: name.trim(),
      filters: [
        ...filters.map((filter) => ({
          column: filter.column,
          op: filter.op as SheetView['filters'][number]['op'],
          ...(NO_VALUE.has(filter.op) ? {} : { value: filter.value.trim() !== '' && Number.isFinite(Number(filter.value)) ? Number(filter.value) : filter.value }),
        })),
        ...listFilters,
      ],
      sort: sortColumn ? [{ column: sortColumn, descending }] : [],
      hiddenColumns,
    }
    setBusy(true)
    try {
      const saved = view
        ? await callOp('sheet_view_update', { viewId: view.id, ...body }, { expectedVersion: view.version })
        : await callOp('sheet_view_create', { sheetId, tab: tabId, shared: canShare && shared, ...body })
      await client.invalidateQueries({ queryKey: opPrefix('sheet_view_list') })
      onSaved(saved)
      onClose()
    } catch (error) {
      setProblem(describeError(error).message)
      setBusy(false)
    }
  }

  return (
    <Dialog
      description="A view changes what you see, not the sheet: the rows that show, their order and the columns that hide."
      dirty
      footer={
        <>
          <Button onClick={onClose} variant="ghost">
            Cancel
          </Button>
          <Button busy={busy} type="submit" variant="primary">
            {view ? 'Save view' : 'Make view'}
          </Button>
        </>
      }
      onClose={onClose}
      onSubmit={(event) => {
        event.preventDefault()
        void submit()
      }}
      open
      title={view ? 'Edit view' : 'New filter view'}
      width={560}
    >
      {problem ? <Banner tone="danger">{problem}</Banner> : null}
      <TextField label="Name" maxLength={80} onChange={(event) => setName(event.target.value)} value={name} />
      {!view && canShare ? <Checkbox checked={shared} description="Everyone who can see the sheet can use it." label="Share this view" onChange={setShared} /> : null}
      <fieldset className="aw-sheet__view-filters">
        <legend>Show rows where</legend>
        {filters.map((filter, index) => (
          <div className="aw-sheet__view-filter" key={index}>
            <SelectField aria-label="Column" compact onChange={(event) => setFilters(filters.map((entry, at) => (at === index ? { ...entry, column: event.target.value } : entry)))} value={filter.column}>
              {letters.map((letter) => (
                <option key={letter} value={letter}>
                  Column {letter}
                </option>
              ))}
            </SelectField>
            <SelectField aria-label="Test" compact onChange={(event) => setFilters(filters.map((entry, at) => (at === index ? { ...entry, op: event.target.value } : entry)))} value={filter.op}>
              {OPS.map(([op, label]) => (
                <option key={op} value={op}>
                  {label}
                </option>
              ))}
            </SelectField>
            {NO_VALUE.has(filter.op) ? null : (
              <TextField aria-label="Value" compact onChange={(event) => setFilters(filters.map((entry, at) => (at === index ? { ...entry, value: event.target.value } : entry)))} value={filter.value} />
            )}
            <IconButton icon="x" label="Remove this filter" onClick={() => setFilters(filters.filter((_, at) => at !== index))} size="sm" />
          </div>
        ))}
        <Button icon="plus" onClick={() => setFilters([...filters, { column: 'A', op: 'contains', value: '' }])} size="sm" variant="ghost">
          Add a filter
        </Button>
      </fieldset>
      <div className="aw-sheet__view-filter">
        <SelectField compact label="Order by" onChange={(event) => setSortColumn(event.target.value)} value={sortColumn}>
          <option value="">Sheet order</option>
          {letters.map((letter) => (
            <option key={letter} value={letter}>
              Column {letter}
            </option>
          ))}
        </SelectField>
        {sortColumn ? <Checkbox checked={descending} label="Largest or last first" onChange={setDescending} /> : null}
      </div>
      <TextField hint="Their letters, such as C, E." label="Hide columns" onChange={(event) => setHidden(event.target.value)} value={hidden} />
    </Dialog>
  )
}

const TESTS: [string, string, 'value' | 'two' | 'text' | 'formula' | 'rank' | 'none' | 'scale'][] = [
  ['lessThan', 'Less than', 'value'],
  ['greaterThan', 'More than', 'value'],
  ['atMost', 'At most', 'value'],
  ['atLeast', 'At least', 'value'],
  ['equals', 'Equal to', 'value'],
  ['notEquals', 'Not equal to', 'value'],
  ['between', 'Between', 'two'],
  ['textContains', 'Text contains', 'text'],
  ['textDoesNotContain', 'Text doesn’t contain', 'text'],
  ['textBeginsWith', 'Text begins with', 'text'],
  ['textEndsWith', 'Text ends with', 'text'],
  ['empty', 'Is empty', 'none'],
  ['notEmpty', 'Isn’t empty', 'none'],
  ['errors', 'Shows an error', 'none'],
  ['top', 'Among the largest', 'rank'],
  ['bottom', 'Among the smallest', 'rank'],
  ['aboveAverage', 'Above average', 'none'],
  ['belowAverage', 'Below average', 'none'],
  ['duplicates', 'Appears more than once', 'none'],
  ['formula', 'Formula is true', 'formula'],
  ['colorScale', 'Colour scale', 'scale'],
]
const LOOKS: [string, NonNullable<ConditionalRule['style']>][] = [
  ['Red text', { color: '#C62828' }],
  ['Green text', { color: '#2E7D32' }],
  ['Bold red text', { bold: true, color: '#C62828' }],
  ['Red fill', { fill: '#FDECEA' }],
  ['Green fill', { fill: '#E6F4EA' }],
  ['Yellow fill', { fill: '#FFF4CC' }],
  ['Blue fill', { fill: '#E3F2FD' }],
  ['Bold', { bold: true }],
  ['Struck through', { strike: true }],
]
const SCALES: [string, { low: string; high: string; mid?: string }][] = [
  ['White to green', { low: '#FFFFFF', high: '#57BB8A' }],
  ['Red to green', { low: '#E67C73', mid: '#FFD666', high: '#57BB8A' }],
  ['Green to red', { low: '#57BB8A', mid: '#FFD666', high: '#E67C73' }],
]
const sameLook = (left: ConditionalRule['style'], right: ConditionalRule['style']) => JSON.stringify(left ?? {}) === JSON.stringify(right ?? {})
const asValue = (text: string): string | number => (text.trim() !== '' && Number.isFinite(Number(text)) ? Number(text) : text)

/** A tab's conditional formats: a list of rules, the first that applies to a cell winning. */
export function SheetRulesDialog({
  onClose,
  onSave,
  selection,
  tab,
}: {
  onClose: () => void
  onSave: (rules: ConditionalRule[]) => Promise<boolean>
  /** The selected cells, such as `C2:C40`, for a new rule's range. */
  selection: string
  tab: SheetTab
}) {
  const [rules, setRules] = useState<ConditionalRule[]>(tab.conditionalFormats)
  const [busy, setBusy] = useState(false)
  const set = (index: number, change: Partial<ConditionalRule>) => setRules(rules.map((rule, at) => (at === index ? { ...rule, ...change } : rule)))
  const setTest = (index: number, when: string) => {
    const kind = TESTS.find(([id]) => id === when)?.[2]
    const { range, style } = rules[index]
    const base: ConditionalRule = { range, when }
    if (kind === 'value') base.value = 0
    if (kind === 'two') Object.assign(base, { value: 0, value2: 10 })
    if (kind === 'text') base.text = ''
    if (kind === 'formula') base.formula = '='
    if (kind === 'rank') base.rank = 10
    if (kind === 'scale') Object.assign(base, SCALES[0][1])
    else base.style = style ?? LOOKS[0][1]
    setRules(rules.map((rule, at) => (at === index ? base : rule)))
  }
  const save = async () => {
    setBusy(true)
    const saved = await onSave(rules)
    setBusy(false)
    if (saved) onClose()
  }
  return (
    <Dialog
      description="Rules colour cells by what they hold. When several apply to a cell, the first one wins."
      dirty
      footer={
        <>
          <Button onClick={onClose} variant="ghost">
            Cancel
          </Button>
          <Button busy={busy} type="submit" variant="primary">
            Save rules
          </Button>
        </>
      }
      onClose={onClose}
      onSubmit={(event) => {
        event.preventDefault()
        void save()
      }}
      open
      title={`Conditional formatting: ${tab.name}`}
      width={640}
    >
      <div className="aw-sheet__view-filters">
        {rules.length === 0 ? <p className="aw-sheet__rules-empty">No rules yet.</p> : null}
        {rules.map((rule, index) => {
          const kind = TESTS.find(([id]) => id === rule.when)?.[2] ?? 'none'
          return (
            <div className="aw-sheet__view-filter" key={index}>
              <TextField aria-label="Cells" compact onChange={(event) => set(index, { range: event.target.value.toUpperCase() })} value={rule.range} />
              <SelectField aria-label="When" compact onChange={(event) => setTest(index, event.target.value)} value={rule.when}>
                {TESTS.map(([id, label]) => (
                  <option key={id} value={id}>
                    {label}
                  </option>
                ))}
              </SelectField>
              {kind === 'value' || kind === 'two' ? (
                <TextField aria-label="Value" compact onChange={(event) => set(index, { value: asValue(event.target.value) })} value={String(rule.value ?? '')} />
              ) : null}
              {kind === 'two' ? <TextField aria-label="And" compact onChange={(event) => set(index, { value2: asValue(event.target.value) })} value={String(rule.value2 ?? '')} /> : null}
              {kind === 'text' ? <TextField aria-label="Text" compact onChange={(event) => set(index, { text: event.target.value })} value={rule.text ?? ''} /> : null}
              {kind === 'formula' ? <TextField aria-label="Formula" compact mono onChange={(event) => set(index, { formula: event.target.value })} value={rule.formula ?? ''} /> : null}
              {kind === 'rank' ? <TextField aria-label="How many" compact inputMode="numeric" onChange={(event) => set(index, { rank: Number(event.target.value) || 1 })} value={String(rule.rank ?? 10)} /> : null}
              {kind === 'scale' ? (
                <SelectField
                  aria-label="Colours"
                  compact
                  onChange={(event) => {
                    const scale = SCALES.find(([label]) => label === event.target.value)?.[1]
                    if (scale) set(index, { low: scale.low, high: scale.high, mid: scale.mid })
                  }}
                  value={SCALES.find(([, scale]) => scale.low === rule.low && scale.high === rule.high)?.[0] ?? ''}
                >
                  {SCALES.map(([label]) => (
                    <option key={label} value={label}>
                      {label}
                    </option>
                  ))}
                </SelectField>
              ) : (
                <SelectField
                  aria-label="Look"
                  compact
                  onChange={(event) => set(index, { style: LOOKS.find(([label]) => label === event.target.value)?.[1] })}
                  value={LOOKS.find(([, look]) => sameLook(look, rule.style))?.[0] ?? ''}
                >
                  {LOOKS.find(([, look]) => sameLook(look, rule.style)) ? null : <option value="">Its own look</option>}
                  {LOOKS.map(([label]) => (
                    <option key={label} value={label}>
                      {label}
                    </option>
                  ))}
                </SelectField>
              )}
              <IconButton icon="x" label="Remove this rule" onClick={() => setRules(rules.filter((_, at) => at !== index))} size="sm" />
            </div>
          )
        })}
        <Button icon="plus" onClick={() => setRules([...rules, { range: selection, when: 'lessThan', value: 0, style: LOOKS[0][1] }])} size="sm" variant="ghost">
          Add a rule
        </Button>
      </div>
    </Dialog>
  )
}
