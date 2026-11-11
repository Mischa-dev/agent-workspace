import { Navigate, Route, Routes, useParams } from 'react-router-dom'

import { CollectionPage } from './CollectionPage'
import { CollectionsListPage } from './CollectionsListPage'
import './collections.css'

/**
 * A project's Collections area (AWD-90), one lazy chunk: the list of its
 * collections, and one collection with its views, records and a record
 * beside them.
 */
export default function CollectionsArea() {
  return (
    <Routes>
      <Route element={<CollectionsListPage />} index />
      <Route element={<CollectionPage />} path=":collectionId" />
      <Route element={<BackToList />} path="*" />
    </Routes>
  )
}

function BackToList() {
  const { projectId = '' } = useParams()
  return <Navigate replace to={`/projects/${encodeURIComponent(projectId)}/collections`} />
}
