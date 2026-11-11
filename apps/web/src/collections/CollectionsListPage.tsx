import { useState } from 'react'
import { useQueryClient } from '@tanstack/react-query'
import { Link, useNavigate, useParams } from 'react-router-dom'

import { useAuthenticatedSession } from '../auth/session'
import { ProjectPageError, ProjectPageLoading } from '../projects/ProjectPageStates'
import { useProject } from '../projects/hooks'
import { AppShell } from '../shell/AppShell'
import { Button, EmptyState, ErrorState, Icon, ListSkeleton, PageHeader, Row, RowList, SegmentedControl, countLabel, describeError, formatRelative, showToast } from '../ui'
import { collectionKeys, updateCollection, useCollections, type CollectionSummary } from './api'
import { CollectionDetailsDialog } from './CollectionDialogs'

type Shown = 'open' | 'archived'

/**
 * A project's collections: the record types it keeps for itself, such as
 * Playtest feedback or Builds, each with how many records it has. People who
 * build collections make new ones here and restore archived ones.
 */
export function CollectionsListPage() {
  const session = useAuthenticatedSession()
  const { projectId = '' } = useParams()
  const project = useProject(projectId)
  const collections = useCollections(projectId, true)
  const client = useQueryClient()
  const navigate = useNavigate()
  const [shown, setShown] = useState<Shown>('open')
  const [creating, setCreating] = useState(false)

  if (project.isPending) {
    return (
      <AppShell user={session.user}>
        <ProjectPageLoading label="Loading collections" />
      </AppShell>
    )
  }
  if (project.isError) {
    return (
      <AppShell user={session.user}>
        <ProjectPageError error={project.error} onRetry={() => void project.refetch()} />
      </AppShell>
    )
  }

  const canBuild = Boolean(collections.data?.canBuild) && !project.data.archivedAt
  const all = collections.data?.collections ?? []
  const archivedCount = all.filter((collection) => collection.archived).length
  const visible = all.filter((collection) => (shown === 'archived' ? collection.archived : !collection.archived))
  const href = (collection: CollectionSummary) => `/projects/${encodeURIComponent(projectId)}/collections/${encodeURIComponent(collection.id)}`
  const restore = async (collection: CollectionSummary) => {
    try {
      await updateCollection(projectId, collection.id, { expectedVersion: collection.version, archived: false })
      void client.invalidateQueries({ queryKey: collectionKeys.all(projectId) })
      showToast({ tone: 'success', title: `${collection.name} restored` })
    } catch (error) {
      showToast({ tone: 'danger', title: `${collection.name} wasn’t restored`, description: describeError(error).message })
    }
  }
  const newCollection = canBuild ? (
    <Button icon="plus" onClick={() => setCreating(true)} variant="primary">
      New collection
    </Button>
  ) : null

  return (
    <AppShell project={project.data} user={session.user}>
      <div className="aw-page aw-collections">
        <PageHeader actions={newCollection} icon="table" title="Collections" />
        {/* It stays while Archived is chosen, so restoring the last one leaves a way back. */}
        {archivedCount || shown === 'archived' ? (
          <SegmentedControl<Shown>
            label="Collections to show"
            onChange={setShown}
            options={[
              { value: 'open', label: 'Open' },
              { value: 'archived', label: `Archived (${archivedCount})` },
            ]}
            value={shown}
          />
        ) : null}
        {collections.isPending ? <ListSkeleton label="Loading collections" rows={3} /> : null}
        {collections.isError ? <ErrorState error={collections.error} onRetry={() => void collections.refetch()} title="Collections didn’t load" /> : null}
        {collections.isSuccess && visible.length === 0 ? (
          <EmptyState action={shown === 'archived' ? undefined : newCollection} icon="table" title={shown === 'archived' ? 'No archived collections' : 'No collections yet'}>
            {shown === 'archived'
              ? 'Archived collections show up here.'
              : 'A collection is a list of things this project keeps track of, such as playtest feedback, builds or recipes, with the fields you choose. People and agents add to it.'}
          </EmptyState>
        ) : null}
        {visible.length ? (
          <RowList label="Collections">
            {visible.map((collection) => (
              <Row
                actions={
                  collection.archived && canBuild ? (
                    <Button onClick={() => void restore(collection)} size="sm" variant="ghost">
                      Restore
                    </Button>
                  ) : null
                }
                key={collection.id}
                leading={<Icon name="table" size={18} />}
                meta={[countLabel(collection.records, 'record'), collection.description || null, collection.updatedAt ? `changed ${formatRelative(collection.updatedAt)}` : null]
                  .filter(Boolean)
                  .join(' · ')}
                title={<Link to={href(collection)}>{collection.name}</Link>}
              />
            ))}
          </RowList>
        ) : null}
      </div>
      {creating ? (
        <CollectionDetailsDialog collection={null} onClose={() => setCreating(false)} onSaved={(body) => void navigate(href(body.collection))} projectId={projectId} />
      ) : null}
    </AppShell>
  )
}
