import { Link } from 'react-router-dom'

import { isApiError } from '../api/errors'
import { Button, Icon, ListSkeleton, countLabel } from '../ui'
import { useSheet } from './api'
import { cellKey, cellMap, columnName, display, parseRange, rangeName, type Range } from './sheetModel'
import '../collections/collectionEmbed.css'
import './sheetEmbed.css'

/** The most rows and columns an embed shows. */
const ROWS = 20
const COLUMNS = 8

export type SheetEmbedProps = { projectId: string; sheetId: string; tabId: string | null; range: string | null; label: string }

/**
 * A sheet's range inside a page (AWD-54): the values, read only, opening the
 * sheet. It loads as whoever reads the page, and refetches when the sheet
 * changes (`sheet.updated`). Without a range it shows the tab from A1.
 */
export default function SheetEmbed({ label, projectId, range, sheetId, tabId }: SheetEmbedProps) {
  const sheet = useSheet(projectId, sheetId, tabId ?? '')
  const body = sheet.data
  const name = label || 'A sheet'

  if (sheet.isPending) return <ListSkeleton label={`Loading ${name}`} rows={3} />
  if (!body) {
    const hidden = isApiError(sheet.error) && (sheet.error.status === 404 || sheet.error.status === 403)
    return (
      <p className="aw-cembed__problem">
        <Icon name="table" size={16} />
        <span>{hidden ? `${name} isn’t here, or you can’t see it.` : `${name} didn’t load.`}</span>
        {hidden ? null : (
          <Button onClick={() => void sheet.refetch()} size="sm">
            Try again
          </Button>
        )}
      </p>
    )
  }

  const tab = body.tabs.find((entry) => entry.id === body.tab) ?? null
  const asked = range ? parseRange(range) : null
  const whole: Range = { top: 1, left: 1, bottom: Math.max(tab?.rows ?? 0, 1), right: Math.max(tab?.columns ?? 0, 1) }
  const area = asked ?? whole
  const shown: Range = { ...area, bottom: Math.min(area.bottom, area.top + ROWS - 1), right: Math.min(area.right, area.left + COLUMNS - 1) }
  const cells = cellMap(body.cells)
  // The tab's frozen header names the columns when the range starts with it.
  const header = Boolean(tab && tab.frozenRows > 0 && shown.top === 1 && shown.bottom > 1)
  const columns = Array.from({ length: shown.right - shown.left + 1 }, (_, index) => shown.left + index)
  const rows = Array.from({ length: shown.bottom - shown.top + 1 - (header ? 1 : 0) }, (_, index) => shown.top + (header ? 1 : 0) + index)
  const moreRows = area.bottom - shown.bottom
  const moreColumns = area.right - shown.right
  const open = `/projects/${encodeURIComponent(projectId)}/sheets/${encodeURIComponent(sheetId)}${tab ? `?tab=${encodeURIComponent(tab.id)}` : ''}`
  const empty = body.cells.length === 0 || rows.every((row) => columns.every((column) => !cells.has(cellKey(row, column))))

  return (
    <div className="aw-cembed">
      <div className="aw-cembed__head">
        <Icon name="table" size={16} />
        <Link className="aw-cembed__name" to={open}>
          {body.sheet.name}
        </Link>
        {tab && body.tabs.length > 1 ? <span className="aw-cembed__view">{tab.name}</span> : null}
        {range ? <span className="aw-cembed__count">{asked ? rangeName(asked) : `“${range}” isn’t a range`}</span> : null}
      </div>
      {empty && !header ? (
        <p className="aw-cembed__empty">Nothing here yet.</p>
      ) : (
        <div className="aw-cembed__scroll">
          <table className="aw-cembed__table">
            <thead>
              <tr>
                {columns.map((column) => (
                  <th key={column} scope="col">
                    <span className="aw-cembed__clip">{(header && display(cells.get(cellKey(1, column)))) || columnName(column)}</span>
                  </th>
                ))}
              </tr>
            </thead>
            <tbody>
              {rows.map((row) => (
                <tr key={row}>
                  {columns.map((column) => {
                    const cell = cells.get(cellKey(row, column))
                    return (
                      <td className={cell?.kind === 'number' ? 'aw-cembed__number' : undefined} key={column}>
                        <span className="aw-cembed__clip">{display(cell)}</span>
                      </td>
                    )
                  })}
                </tr>
              ))}
            </tbody>
          </table>
        </div>
      )}
      {moreRows > 0 || moreColumns > 0 ? (
        <Link className="aw-cembed__more" to={open}>
          {[moreRows > 0 ? countLabel(moreRows, 'more row') : '', moreColumns > 0 ? countLabel(moreColumns, 'more column') : ''].filter(Boolean).join(' and ')} in{' '}
          {body.sheet.name}
        </Link>
      ) : null}
    </div>
  )
}
