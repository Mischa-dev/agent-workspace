import type { LiveHandler } from '../modules/define'
import { collectionChange, COLLECTION_DELAY_MS, later, refreshDashboards } from '../realtime/liveDocuments'
import { collectionKeys } from './keys'

/**
 * A collection.updated event (M1.5): everything open of the collection
 * refetches (its records, views and fields, and a record beside them) once
 * for a burst of changes, and so does the space's list of collections, which
 * counts records, and any open dashboard, whose tiles may read it.
 */
const collectionChanged: LiveHandler = (queryClient, event) => {
  const collection = collectionChange(event)
  if (!collection) return
  later(`collection:${event.projectId}:${collection.collectionId}`, COLLECTION_DELAY_MS, () => {
    void queryClient.invalidateQueries({ queryKey: collectionKeys.one(event.projectId, collection.collectionId) })
    void queryClient.invalidateQueries({ queryKey: collectionKeys.lists(event.projectId) })
    refreshDashboards(queryClient)
  })
}

export default collectionChanged
