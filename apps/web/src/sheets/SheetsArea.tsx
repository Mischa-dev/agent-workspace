import { Navigate, Route, Routes, useParams } from 'react-router-dom'

import { SheetPage } from './SheetPage'
import { SheetsListPage } from './SheetsListPage'
import './sheets.css'

/**
 * A project's Sheets area (AWD-54), one lazy chunk: the list of its sheets,
 * and one sheet as a grid you edit in place.
 */
export default function SheetsArea() {
  return (
    <Routes>
      <Route element={<SheetsListPage />} index />
      <Route element={<SheetPage />} path=":sheetId" />
      <Route element={<BackToList />} path="*" />
    </Routes>
  )
}

function BackToList() {
  const { projectId = '' } = useParams()
  return <Navigate replace to={`/projects/${encodeURIComponent(projectId)}/sheets`} />
}
