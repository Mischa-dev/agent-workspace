import { Navigate, Route, Routes, useParams } from 'react-router-dom'

import { DashboardPage } from './DashboardPage'
import { DashboardsListPage } from './DashboardsListPage'
import { OutsideDataPage } from './OutsideDataPage'
import { dashboardsHref } from './dashboardModel'
import './dashboards.css'

/**
 * A project's Dashboards area (T1.32), one lazy chunk: the list of its
 * dashboards, one dashboard as a grid of tiles read live, and the outside
 * data its query tiles show (T1.34).
 */
export default function DashboardsArea() {
  return (
    <Routes>
      <Route element={<DashboardsListPage />} index />
      <Route element={<OutsideDataPage />} path="data" />
      <Route element={<DashboardPage />} path=":dashboardId" />
      <Route element={<BackToList />} path="*" />
    </Routes>
  )
}

function BackToList() {
  const { projectId = '' } = useParams()
  return <Navigate replace to={dashboardsHref(projectId)} />
}
