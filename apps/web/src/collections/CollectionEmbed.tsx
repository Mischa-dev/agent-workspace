import { Link } from 'react-router-dom'

import { isApiError } from '../api/errors'
import { FieldValueView } from '../fields/FieldValueView'
import { Button, Icon, ListSkeleton, countLabel } from '../ui'
import { recordsParams, useCollection, useRecords, type CollectionView } from './api'
import { activeConfig, recordName, viewColumns } from './collectionModel'
import './collectionEmbed.css'

/** The most records an embed lists, and the most columns beside the title. */
const ROWS = 8
const COLUMNS = 4

export type CollectionEmbedProps = { projectId: string; collectionId: string; viewId: string | null; label: string }

/**
 * A collection's view inside a page (AWD-90): its first records, read only,
 * each opening in the collection. It loads as whoever reads the page, so it
 * shows only what they may see, and refetches when the records change.
 */
export default function CollectionEmbed({ collectionId, label, projectId, viewId }: CollectionEmbedProps) {
  const collection = useCollection(projectId, collectionId)
  const body = collection.data
  const views = body ? [...body.views].sort((left, right) => left.position - right.position) : []
  const view: CollectionView | null = views.find((entry) => entry.id === viewId) ?? views[0] ?? null
  const records = useRecords(projectId, collectionId, recordsParams({ view: view?.id ?? null }), Boolean(body))
  const name = label || 'A collection'

  if (collection.isPending) return <ListSkeleton label={`Loading ${name}`} rows={3} />
  if (!body) {
    const hidden = isApiError(collection.error) && (collection.error.status === 404 || collection.error.status === 403)
    return (
      <p className="aw-cembed__problem">
        <Icon name="table" size={16} />
        <span>{hidden ? `${name} isn’t here, or you can’t see it.` : `${name} didn’t load.`}</span>
        {hidden ? null : (
          <Button onClick={() => void collection.refetch()} size="sm">
            Try again
          </Button>
        )}
      </p>
    )
  }

  const base = `/projects/${encodeURIComponent(projectId)}/collections/${encodeURIComponent(collectionId)}`
  const open = view ? `${base}?view=${encodeURIComponent(view.id)}` : base
  const columns = view ? viewColumns(body.fields, view.config).slice(0, COLUMNS) : []
  // A filter on an archived field doesn't count, as the server leaves it out.
  const filtered = view ? activeConfig(view.config, body.fields).filters.length > 0 : false
  const page = records.data?.pages[0]
  const shown = page?.records.slice(0, ROWS) ?? []
  const total = page?.total ?? 0
  const references = page?.references ?? []

  return (
    <div className="aw-cembed">
      <div className="aw-cembed__head">
        <Icon name="table" size={16} />
        <Link className="aw-cembed__name" to={open}>
          {body.collection.name}
        </Link>
        {view && views.length > 1 ? <span className="aw-cembed__view">{view.name}</span> : null}
        {page ? <span className="aw-cembed__count">{countLabel(total, 'record')}</span> : null}
      </div>
      {records.isPending ? (
        <ListSkeleton decorative rows={3} />
      ) : !page ? (
        <p className="aw-cembed__problem">
          <span>The records didn’t load.</span>
          <Button onClick={() => void records.refetch()} size="sm">
            Try again
          </Button>
        </p>
      ) : shown.length === 0 ? (
        <p className="aw-cembed__empty">{filtered ? 'Nothing in this view yet.' : 'No records yet.'}</p>
      ) : (
        <div className="aw-cembed__scroll">
          <table className="aw-cembed__table">
            <thead>
              <tr>
                <th scope="col">{body.collection.titleName || 'Name'}</th>
                {columns.map(({ field }) => (
                  <th key={field.id} scope="col">
                    {field.name}
                  </th>
                ))}
              </tr>
            </thead>
            <tbody>
              {shown.map((record) => (
                <tr key={record.id}>
                  <th scope="row">
                    <Link className="aw-cembed__clip" to={`${open}${open.includes('?') ? '&' : '?'}record=${encodeURIComponent(record.id)}`}>
                      {recordName(record)}
                    </Link>
                  </th>
                  {columns.map(({ field }) => (
                    <td key={field.id}>
                      <div className="aw-cembed__clip">
                        <FieldValueView field={field} references={references} value={record.values[field.id]} />
                      </div>
                    </td>
                  ))}
                </tr>
              ))}
            </tbody>
          </table>
        </div>
      )}
      {page && total > shown.length ? (
        <Link className="aw-cembed__more" to={open}>
          {countLabel(total - shown.length, 'more record')} in {body.collection.name}
        </Link>
      ) : null}
    </div>
  )
}
