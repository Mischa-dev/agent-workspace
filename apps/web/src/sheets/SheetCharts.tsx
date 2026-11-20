// Charts on a sheet (C1.18): a block of the sheet drawn as bars, a line, a
// donut or one number, worked out on the server from the cells as they are.
// The sheet shows its charts in a dialog where editors add, change and
// remove them; a page embeds one (SheetChartEmbed).
import { useState } from 'react'
import { useQuery } from '@tanstack/react-query'

import { callOp, opQuery } from '../api/ops'
import { ChartFigure } from '../collections/CollectionChartView'
import { chartNumber } from '../collections/collectionModel'
import { Banner, Button, Dialog, EmptyState, IconButton, ListSkeleton, SelectField, TextField, describeError } from '../ui'
import type { SheetChart } from './api'
import './sheetChart.css'

const STYLES: [SheetChart['style'], string][] = [
  ['bar', 'Bars'],
  ['line', 'A line'],
  ['donut', 'A donut'],
  ['number', 'One number'],
]

/** One chart, drawn from its figures now. */
export function SheetChartFigure({ chart, sheetId }: { chart: Pick<SheetChart, 'id' | 'title' | 'style'>; sheetId: string }) {
  const figures = useQuery(opQuery('sheet_chart_get', { sheetId, chartId: chart.id, withoutPicture: true }))
  if (figures.isPending) return <ListSkeleton label={`Loading ${chart.title}`} rows={3} />
  if (figures.isError) return <p className="aw-sheet-chart__note">{describeError(figures.error).message}</p>
  const data = figures.data
  if (!data.found) return <p className="aw-sheet-chart__note">What this chart draws isn’t in the sheet any more. Choose its cells again.</p>
  const label = data.measure ?? 'Value'
  if (data.style === 'number' || data.points.length === 0) {
    return (
      <figure className="aw-sheet-chart__number">
        <span>{data.total === undefined || data.total === null ? '—' : chartNumber(data.total)}</span>
        <figcaption>{label}</figcaption>
      </figure>
    )
  }
  const points = data.points.map((point, index) => ({ key: `${index}:${point.label}`, label: point.label || 'None', color: null, count: 1, value: point.value }))
  return <ChartFigure additive label={label} points={points} style={data.style === 'line' || data.style === 'donut' ? data.style : 'bar'} />
}

/** A sheet's charts: each drawn, and for editors the means to add, change and remove them. */
export function SheetChartsDialog({
  canEdit,
  charts,
  onChanged,
  onClose,
  selection,
  sheetId,
}: {
  canEdit: boolean
  charts: SheetChart[]
  onChanged: () => void
  onClose: () => void
  /** The selected cells, such as `Costs!A1:B9`, for a new chart's source. */
  selection: string
  sheetId: string
}) {
  const [editing, setEditing] = useState<{ id: string | null; title: string; style: SheetChart['style']; source: string } | null>(null)
  const [busy, setBusy] = useState(false)
  const [problem, setProblem] = useState<string | null>(null)

  const save = async () => {
    if (!editing) return
    setBusy(true)
    setProblem(null)
    try {
      await callOp('sheet_chart_set', {
        sheetId,
        ...(editing.id ? { chartId: editing.id } : {}),
        title: editing.title.trim() || editing.source,
        style: editing.style,
        source: editing.source,
      })
      setEditing(null)
      onChanged()
    } catch (error) {
      setProblem(describeError(error).message)
    } finally {
      setBusy(false)
    }
  }
  const remove = async (chart: SheetChart) => {
    setBusy(true)
    setProblem(null)
    try {
      await callOp('sheet_chart_set', { sheetId, chartId: chart.id, remove: true })
      onChanged()
    } catch (error) {
      setProblem(describeError(error).message)
    } finally {
      setBusy(false)
    }
  }

  return (
    <Dialog
      description="Charts draw a block of the sheet: labels in its first column, numbers in its second. Chart a named range to keep it in place as rows move. A page can embed any of them."
      footer={
        <Button onClick={onClose} variant="ghost">
          Close
        </Button>
      }
      onClose={onClose}
      open
      title="Charts"
      width={760}
    >
      {problem ? <Banner tone="danger">{problem}</Banner> : null}
      {editing ? (
        <form
          aria-label={editing.id ? 'Change the chart' : 'Add a chart'}
          className="aw-sheet-chart__form"
          onSubmit={(event) => {
            event.preventDefault()
            void save()
          }}
        >
          <TextField compact label="Title" onChange={(event) => setEditing({ ...editing, title: event.target.value })} value={editing.title} />
          <TextField compact label="Cells" onChange={(event) => setEditing({ ...editing, source: event.target.value })} placeholder="A named range, or such as Costs!A1:B9" value={editing.source} />
          <SelectField compact label="Draw as" onChange={(event) => setEditing({ ...editing, style: event.target.value as SheetChart['style'] })} value={editing.style}>
            {STYLES.map(([id, label]) => (
              <option key={id} value={id}>
                {label}
              </option>
            ))}
          </SelectField>
          <span className="aw-sheet-chart__actions">
            <Button busy={busy} disabled={!editing.source.trim()} size="sm" type="submit" variant="primary">
              {editing.id ? 'Save chart' : 'Add chart'}
            </Button>
            <Button onClick={() => setEditing(null)} size="sm" variant="ghost">
              Cancel
            </Button>
          </span>
        </form>
      ) : canEdit ? (
        <Button icon="plus" onClick={() => setEditing({ id: null, title: '', style: 'bar', source: selection })} size="sm" variant="ghost">
          Add a chart
        </Button>
      ) : null}
      {charts.length === 0 && !editing ? (
        <EmptyState icon="chart" title="No charts yet">
          {canEdit ? 'Select some cells and add a chart of them.' : 'Someone who edits this sheet can add one.'}
        </EmptyState>
      ) : null}
      <div className="aw-sheet-chart__list">
        {charts.map((chart) => (
          <section aria-label={chart.title} className="aw-sheet-chart" key={chart.id}>
            <header className="aw-sheet-chart__head">
              <h3>{chart.title}</h3>
              <span className="aw-sheet-chart__source">{chart.source}</span>
              {canEdit ? (
                <>
                  <IconButton icon="pencil" label={`Change ${chart.title}`} onClick={() => setEditing({ id: chart.id, title: chart.title, style: chart.style, source: chart.source })} size="sm" />
                  <IconButton disabled={busy} icon="trash" label={`Remove ${chart.title}`} onClick={() => void remove(chart)} size="sm" />
                </>
              ) : null}
            </header>
            <SheetChartFigure chart={chart} sheetId={sheetId} />
          </section>
        ))}
      </div>
    </Dialog>
  )
}
