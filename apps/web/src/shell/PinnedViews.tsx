import { useQuery } from '@tanstack/react-query'
import { NavLink, useLocation } from 'react-router-dom'

import { Icon } from '../ui'
import { listViews, viewKeys } from '../views/api'
import { SidebarSection } from './SidebarParts'

/**
 * The Project's pinned views in its sidebar, so Ready for agents and the
 * views you use most are one click away. Loaded on its own after the shell.
 */
export default function PinnedViews({ onNavigate, projectId }: { onNavigate: () => void; projectId: string }) {
  const location = useLocation()
  const views = useQuery({ queryKey: viewKeys.views(projectId), queryFn: () => listViews(projectId), staleTime: 60_000 })
  const pinned = (views.data?.views ?? []).filter((view) => view.pinned).sort((left, right) => left.position - right.position)
  if (!pinned.length) return null
  const current = new URLSearchParams(location.search).get('savedView')
  return (
    <SidebarSection label="Views">
      {pinned.map((view) => (
        <li key={view.id}>
          <NavLink
            className={() => ['aw-item', view.id === current && location.pathname.endsWith('/work-items') ? 'is-active' : ''].filter(Boolean).join(' ')}
            onClick={onNavigate}
            to={view.href}
          >
            <span className="aw-item__lead">
              <Icon name={view.builtin === 'ready_for_agents' ? 'bot' : view.layout === 'board' ? 'board' : 'list'} />
            </span>
            <span className="aw-item__label">{view.name}</span>
          </NavLink>
        </li>
      ))}
    </SidebarSection>
  )
}
