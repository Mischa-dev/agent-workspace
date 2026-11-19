import { useState, type FormEvent } from 'react'
import { Link } from 'react-router-dom'

import { useProjectExperience } from '../experience/layout'
import { sheetChartEmbed, sheetEmbed } from '../memory/embeds/embedModel'
import { Button, SelectField, TextField, describeError } from '../ui'
import { useSheet, useSheets } from './api'
import { parseRange, rangeName } from './sheetModel'

/**
 * Picks a sheet, one of its tabs and, if wanted, a range such as `A1:D9` or
 * one of its named ranges (C1.20) to show live in a page (AWD-54), from the
 * page editor's toolbar.
 */
export default function SheetEmbedPicker({
  onClose,
  onPick,
  projectId,
}: {
  onClose: () => void
  onPick: (embed: { href: string; label: string }) => void
  projectId: string
}) {
  const sheets = useSheets(projectId)
  const showsCharts = useProjectExperience(projectId).shows('sheet.charts')
  const [sheetId, setSheetId] = useState('')
  const [tabId, setTabId] = useState('')
  const [range, setRange] = useState('')
  const [named, setNamed] = useState('')
  const [chartId, setChartId] = useState('')
  const [error, setError] = useState<string | null>(null)
  const chosen = useSheet(projectId, sheetId, '')
  // While another sheet loads, the query still holds the last one's data.
  const picked = chosen.data && !chosen.isPlaceholderData && chosen.data.sheet.id === sheetId ? chosen.data : null
  const open = (sheets.data?.sheets ?? []).filter((sheet) => !sheet.archived)
  const tabs = [...(picked?.tabs ?? [])].sort((left, right) => left.position - right.position)
  const tab = tabs.find((entry) => entry.id === tabId) ?? tabs[0] ?? null

  const submit = (event: FormEvent) => {
    event.preventDefault()
    if (!picked) return
    const chosenChart = picked.charts.find((entry) => entry.id === chartId)
    if (chosenChart) {
      onPick(sheetChartEmbed(projectId, picked.sheet, chosenChart))
      return
    }
    const chosenName = picked.ranges.find((entry) => entry.name === named)
    if (chosenName) {
      onPick(sheetEmbed(projectId, picked.sheet, null, tabs.length, null, chosenName.name))
      return
    }
    const parsed = range.trim() ? parseRange(range) : null
    if (range.trim() && !parsed) {
      setError('Write a range such as A1:D9, or leave it empty for the whole tab.')
      return
    }
    onPick(sheetEmbed(projectId, picked.sheet, tab, tabs.length, parsed ? rangeName(parsed) : null))
  }

  return (
    <form
      aria-label="Add a sheet"
      className="aw-live__link"
      onKeyDown={(event) => {
        if (event.key !== 'Escape') return
        event.preventDefault()
        onClose()
      }}
      onSubmit={submit}
    >
      {sheets.isError ? (
        <p className="aw-live__note">
          The sheets didn’t load. {describeError(sheets.error).message}{' '}
          <Button onClick={() => void sheets.refetch()} size="sm">
            Try again
          </Button>
        </p>
      ) : sheets.data && open.length === 0 ? (
        <p className="aw-live__note">
          This project has no sheets yet. <Link to={`/projects/${encodeURIComponent(projectId)}/sheets`}>Make one in Sheets</Link>.
        </p>
      ) : (
        <>
          <SelectField
            autoFocus
            className="aw-live__address"
            compact
            disabled={!sheets.data}
            label="Sheet"
            onChange={(event) => {
              setSheetId(event.target.value)
              setTabId('')
              setNamed('')
              setChartId('')
            }}
            value={sheetId}
          >
            <option value="">{sheets.data ? 'Choose a sheet' : 'Loading…'}</option>
            {open.map((sheet) => (
              <option key={sheet.id} value={sheet.id}>
                {sheet.name}
              </option>
            ))}
          </SelectField>
          {showsCharts && picked && picked.charts.length > 0 ? (
            <SelectField className="aw-live__address" compact label="Chart" onChange={(event) => setChartId(event.target.value)} value={chartId}>
              <option value="">None: show cells</option>
              {picked.charts.map((entry) => (
                <option key={entry.id} value={entry.id}>
                  {entry.title}
                </option>
              ))}
            </SelectField>
          ) : null}
          {picked && picked.ranges.length > 0 && !chartId ? (
            <SelectField className="aw-live__address" compact label="Named range" onChange={(event) => setNamed(event.target.value)} value={named}>
              <option value="">None: pick the cells below</option>
              {picked.ranges.map((entry) => (
                <option key={entry.name} value={entry.name}>
                  {entry.name} ({tabs.length > 1 ? `${entry.tab} ` : ''}
                  {entry.range})
                </option>
              ))}
            </SelectField>
          ) : null}
          {named || chartId ? null : tabs.length > 1 ? (
            <SelectField className="aw-live__address" compact label="Tab" onChange={(event) => setTabId(event.target.value)} value={tab?.id ?? ''}>
              {tabs.map((entry) => (
                <option key={entry.id} value={entry.id}>
                  {entry.name}
                </option>
              ))}
            </SelectField>
          ) : null}
          {named || chartId ? null : (
          <TextField
            className="aw-live__address"
            compact
            error={error ?? undefined}
            label="Range"
            onChange={(event) => {
              setRange(event.target.value)
              setError(null)
            }}
            placeholder="The whole tab, or such as A1:D9"
            value={range}
          />
          )}
        </>
      )}
      <span className="aw-live__actions">
        <Button disabled={!picked} size="sm" type="submit" variant="primary">
          Add to page
        </Button>
        <Button onClick={onClose} size="sm" variant="ghost">
          Cancel
        </Button>
      </span>
    </form>
  )
}
