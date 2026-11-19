import type { ReactNode } from 'react'
import { Link } from 'react-router-dom'

import type { TileEntry } from '../api/generated/operations'
import { ChartFigure } from '../collections/CollectionChartView'
import { chartNumber } from '../collections/collectionModel'
import { Icon, countLabel } from '../ui'
import { additive, figurePoints, measureWords, tileHref } from './dashboardModel'

/** One tile: a view's chart, count or first records, a sheet block's chart, or a note. */
export function DashboardTile({ menu, projectId, tile }: { menu: ReactNode; projectId: string; tile: TileEntry }) {
  const figures = tile.figures ?? null
  const title = tile.title || figures?.viewName || (tile.kind === 'text' ? '' : tile.kind === 'sheet' ? 'Sheet' : 'View')
  const href = tile.kind !== 'text' && !tile.unseen ? tileHref(projectId, tile) : null
  return (
    <section aria-label={title || 'Note'} className={`aw-dash__tile aw-dash__tile--w${tile.width} aw-dash__tile--h${tile.height}`}>
      {title || menu ? (
        <header className="aw-dash__tilehead">
          <div className="aw-dash__tiletitle">
            {title ? <h2>{href ? <Link to={href}>{title}</Link> : title}</h2> : null}
            {figures ? <span className="aw-dash__tilesub">{figures.collectionName}</span> : null}
          </div>
          {menu}
        </header>
      ) : null}
      <div className="aw-dash__tilebody">{body(tile)}</div>
    </section>
  )
}

function body(tile: TileEntry) {
  if (tile.kind === 'text') return <p className="aw-dash__note">{tile.text}</p>
  if (tile.unseen) {
    return (
      <p className="aw-dash__unseen">
        <Icon name="lock" size={16} />
        {tile.unseen}
      </p>
    )
  }
  const figures = tile.figures
  if (!figures) return <p className="aw-dash__unseen">No figures to show.</p>
  if (figures.viewType !== 'chart') {
    return (
      <div className="aw-dash__list">
        <p className="aw-dash__count">
          <span className="aw-dash__number">{figures.total.toLocaleString()}</span> {figures.total === 1 ? 'record' : 'records'}
        </p>
        {figures.records.length ? (
          <ul>
            {figures.records.map((record, index) => (
              <li key={`${index}:${record}`}>{record}</li>
            ))}
          </ul>
        ) : null}
      </div>
    )
  }
  const counting = !figures.measure || figures.measure === 'count'
  const label = measureWords(figures.measure)
  const overall = counting ? figures.total : (figures.value ?? null)
  const style = figures.style ?? 'bar'
  if (style === 'number' || figures.groups.length === 0) {
    return (
      <figure className="aw-dash__hero">
        <span className="aw-dash__number">{overall === null ? '—' : chartNumber(overall)}</span>
        <figcaption>
          {label}
          {counting ? '' : ` across ${countLabel(figures.total, tile.kind === 'sheet' ? 'row' : 'record')}`}
        </figcaption>
      </figure>
    )
  }
  return (
    <div className="aw-dash__chart">
      <ChartFigure additive={additive(figures.measure)} label={label} points={figurePoints(figures)} style={style === 'line' || style === 'donut' ? style : 'bar'} />
    </div>
  )
}
