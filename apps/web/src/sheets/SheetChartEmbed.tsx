import { Link } from 'react-router-dom'

import { Icon } from '../ui'
import { SheetChartFigure } from './SheetCharts'

export type SheetChartEmbedProps = { projectId: string; sheetId: string; chartId: string; label: string }

/**
 * A sheet's chart inside a page (C1.18): drawn from the cells as they are,
 * read as whoever reads the page, and drawn again when the sheet changes.
 */
export default function SheetChartEmbed({ chartId, label, projectId, sheetId }: SheetChartEmbedProps) {
  return (
    <figure className="aw-sheet-chart aw-sheet-chart--embed">
      <figcaption className="aw-sheet-chart__head">
        <Link to={`/projects/${encodeURIComponent(projectId)}/sheets/${encodeURIComponent(sheetId)}`}>
          <Icon name="chart" size={16} />
          <span>{label || 'A chart'}</span>
        </Link>
      </figcaption>
      <SheetChartFigure chart={{ id: chartId, title: label || 'the chart', style: 'bar' }} sheetId={sheetId} />
    </figure>
  )
}
