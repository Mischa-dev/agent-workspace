// Named ranges (C1.20): a name for a block of one tab, such as Totals for
// Costs!A1:B9, that dashboard tiles, page embeds and agents read by name.
// The server moves a range with the rows and columns inserted around it.
import { Suspense, lazy, useState } from 'react'

import { callOp } from '../api/ops'
import { Banner, Button, Dialog, TextField, describeError } from '../ui'
import type { NamedRange } from './api'

const ReferencedIn = lazy(() => import('../modules/references/ReferencedIn'))

/**
 * Lists a sheet's named ranges, which editors add, move and remove, and
 * where the sheet is linked from.
 */
export function SheetRangesDialog({
  canEdit,
  onChanged,
  onClose,
  ranges,
  references,
  selection,
  sheetId,
  tab,
}: {
  canEdit: boolean
  /** Whether to list the pages, tasks and conversations that link to the sheet. */
  references: boolean
  onChanged: () => void
  onClose: () => void
  ranges: NamedRange[]
  /** The cells selected on screen, such as A1:B9. */
  selection: string
  sheetId: string
  /** The tab on screen, whose name the selection is on. */
  tab: string
}) {
  const [name, setName] = useState('')
  const [range, setRange] = useState(selection)
  const [busy, setBusy] = useState<string | null>(null)
  const [problem, setProblem] = useState<string | null>(null)

  const write = async (input: { name: string; range?: string; remove?: boolean }) => {
    setBusy(input.name)
    setProblem(null)
    try {
      await callOp('sheet_range_set', { sheetId, ...input })
      onChanged()
      if (!input.remove) setName('')
    } catch (error) {
      setProblem(describeError(error).message)
    } finally {
      setBusy(null)
    }
  }
  const quoted = /^[A-Za-z0-9_]+$/.test(tab) ? tab : `'${tab.replaceAll("'", "''")}'`

  return (
    <Dialog
      description="Name a block of cells to chart it on a dashboard, show it in a page or let agents read it by name. A name follows its cells when rows or columns are added above or beside them."
      footer={
        <Button onClick={onClose} variant="ghost">
          Close
        </Button>
      }
      onClose={onClose}
      onSubmit={(event) => {
        event.preventDefault()
        if (!name.trim() || !range.trim()) return setProblem('Give the range a name and its cells.')
        void write({ name: name.trim(), range: range.includes('!') ? range.trim() : `${quoted}!${range.trim()}` })
      }}
      open
      title="Named ranges and links"
      width={520}
    >
      {problem ? <Banner tone="danger">{problem}</Banner> : null}
      {ranges.length === 0 ? <p className="aw-sheet__proposals-empty">This sheet has no named ranges yet.</p> : null}
      {ranges.length ? (
        <ul className="aw-sheet__ranges">
          {ranges.map((entry) => (
            <li key={entry.name}>
              <span className="aw-sheet__range-name">{entry.name}</span>
              <span className="aw-sheet__range-where">
                {entry.tab}!{entry.range}
              </span>
              {canEdit ? (
                <Button busy={busy === entry.name} disabled={busy !== null} onClick={() => void write({ name: entry.name, remove: true })} size="sm" variant="ghost">
                  Remove
                </Button>
              ) : null}
            </li>
          ))}
        </ul>
      ) : null}
      {canEdit ? (
        <div className="aw-sheet__range-form">
          <TextField hint="A letter first, then letters, digits, _ and dots." label="Name" maxLength={60} onChange={(event) => setName(event.target.value)} value={name} />
          <TextField hint={`On ${tab} unless you write another tab, such as Costs!A1:B9.`} label="Cells" maxLength={100} onChange={(event) => setRange(event.target.value)} value={range} />
          <Button busy={busy !== null && busy === name.trim()} disabled={busy !== null} size="sm" type="submit" variant="primary">
            {ranges.some((entry) => entry.name.toLowerCase() === name.trim().toLowerCase()) ? 'Move range' : 'Add range'}
          </Button>
        </div>
      ) : null}
      {references ? (
        <Suspense fallback={null}>
          <ReferencedIn id={sheetId} kind="sheet" />
        </Suspense>
      ) : null}
    </Dialog>
  )
}
