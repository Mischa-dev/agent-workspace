import { useState, type FormEvent } from 'react'
import { Link } from 'react-router-dom'

import { collectionEmbed } from '../memory/embeds/embedModel'
import { Button, SelectField, describeError } from '../ui'
import { useCollection, useCollections } from './api'

/**
 * Picks a collection and one of its views to show live in a page (AWD-90),
 * from the page editor's toolbar. `onPick` gets the embed's path and label.
 */
export default function CollectionEmbedPicker({
  onClose,
  onPick,
  projectId,
}: {
  onClose: () => void
  onPick: (embed: { href: string; label: string }) => void
  projectId: string
}) {
  const collections = useCollections(projectId)
  const [collectionId, setCollectionId] = useState('')
  const [viewId, setViewId] = useState('')
  const chosen = useCollection(projectId, collectionId)
  const open = (collections.data?.collections ?? []).filter((collection) => !collection.archived)
  const views = [...(chosen.data?.views ?? [])].sort((left, right) => left.position - right.position)
  const view = views.find((entry) => entry.id === viewId) ?? views[0] ?? null

  const submit = (event: FormEvent) => {
    event.preventDefault()
    if (!chosen.data) return
    onPick(collectionEmbed(projectId, chosen.data.collection, view, views.length))
  }

  return (
    <form
      aria-label="Add a collection view"
      className="aw-live__link"
      onKeyDown={(event) => {
        if (event.key !== 'Escape') return
        event.preventDefault()
        onClose()
      }}
      onSubmit={submit}
    >
      {collections.isError ? (
        <p className="aw-live__note">
          The collections didn’t load. {describeError(collections.error).message}{' '}
          <Button onClick={() => void collections.refetch()} size="sm">
            Try again
          </Button>
        </p>
      ) : collections.data && open.length === 0 ? (
        <p className="aw-live__note">
          This project has no collections yet. <Link to={`/projects/${encodeURIComponent(projectId)}/collections`}>Make one in Collections</Link>.
        </p>
      ) : (
        <>
          <SelectField
            autoFocus
            className="aw-live__address"
            compact
            disabled={!collections.data}
            label="Collection"
            onChange={(event) => {
              setCollectionId(event.target.value)
              setViewId('')
            }}
            value={collectionId}
          >
            <option value="">{collections.data ? 'Choose a collection' : 'Loading…'}</option>
            {open.map((collection) => (
              <option key={collection.id} value={collection.id}>
                {collection.name}
              </option>
            ))}
          </SelectField>
          {views.length > 1 ? (
            <SelectField className="aw-live__address" compact label="View" onChange={(event) => setViewId(event.target.value)} value={view?.id ?? ''}>
              {views.map((entry) => (
                <option key={entry.id} value={entry.id}>
                  {entry.name}
                </option>
              ))}
            </SelectField>
          ) : null}
        </>
      )}
      <span className="aw-live__actions">
        <Button disabled={!chosen.data} size="sm" type="submit" variant="primary">
          Add to page
        </Button>
        <Button onClick={onClose} size="sm" variant="ghost">
          Cancel
        </Button>
      </span>
    </form>
  )
}
