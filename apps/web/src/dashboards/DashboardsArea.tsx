import { Navigate, Route, Routes, useParams } from 'react-router-dom'

import { DashboardPage } from './DashboardPage'
import { DashboardsListPage } from './DashboardsListPage'
import { dashboardsHref } from './dashboardModel'
import './dashboards.css'

/**
 * A project's Dashboards area (T1.32), one lazy chunk: the list of its
 * dashboards, and one dashboard as a grid of tiles read live.
 */
export default function DashboardsArea() {
  return (
    <Routes>
      <Route element={<DashboardsListPage />} index />
      <Route element={<DashboardPage />} path=":dashboardId" />
      <Route element={<BackToList />} path="*" />
    </Routes>
  )
}

function BackToList() {
  const { projectId = '' } = useParams()
  return <Navigate replace to={dashboardsHref(projectId)} />
}
