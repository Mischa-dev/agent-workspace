import { useState, type DragEvent } from 'react'
import { useQuery, useQueryClient } from '@tanstack/react-query'
import { NavLink, useLocation } from 'react-router-dom'

import { toastError } from '../app/toastError'
import { useProjectExperience } from '../experience/layout'
import { Icon } from '../ui'
import { listProjectTaskViews, pinTaskView, taskViewHref, viewKeys, type TaskView } from '../views/api'
import { viewName, viewShown } from '../views/layoutViews'
import { orderAfterDrop } from './dragOrder'
import { isMobileWidth } from './mobileNavigation'
import { SidebarSection } from './SidebarParts'

/** What a dragged view carries, so other drags don't reorder the list. */
const VIEW_DRAG_TYPE = 'application/x-agent-workspace-view'

/**
 * The Project's pinned views in its sidebar, so Ready for agents and the
 * views you use most are one click away, in the order you drag them into.
 * Loaded on its own after the shell.
 */
export default function PinnedViews({ onNavigate, projectId }: { onNavigate: () => void; projectId: string }) {
  const location = useLocation()
  const queryClient = useQueryClient()
  const experience = useProjectExperience(projectId)
  const views = useQuery({ queryKey: viewKeys.views(projectId), queryFn: () => listProjectTaskViews(projectId), staleTime: 60_000 })
  const [drag, setDrag] = useState<{ id: string; over: { id: string; after: boolean } | null } | null>(null)
  const pinned = (views.data?.views ?? [])
    .filter((view) => view.pinned && viewShown(view, experience))
    .sort((left, right) => left.position - right.position)
  if (!pinned.length) return null
  const current = new URLSearchParams(location.search).get('savedView')

  // Your own order: each pin keeps its place on the server, for you alone.
  const reorder = async (order: TaskView[]) => {
    const key = viewKeys.views(projectId)
    const place = new Map(order.map((view, index) => [view.id, index]))
    queryClient.setQueryData<Awaited<ReturnType<typeof listProjectTaskViews>>>(key, (data) =>
      data ? { ...data, views: data.views.map((view) => (place.has(view.id) ? { ...view, position: place.get(view.id) ?? view.position } : view)) } : data,
    )
    try {
      await Promise.all(order.map((view, index) => (view.position === index ? null : pinTaskView(view.id, true, crypto.randomUUID(), index))))
    } catch (reason) {
      toastError('The views’ order wasn’t saved', reason)
    } finally {
      void queryClient.invalidateQueries({ queryKey: key })
    }
  }
  const dragOver = (event: DragEvent<HTMLElement>, view: TaskView) => {
    if (!drag || !event.dataTransfer.types.includes(VIEW_DRAG_TYPE)) return
    event.preventDefault()
    event.dataTransfer.dropEffect = 'move'
    const box = event.currentTarget.getBoundingClientRect()
    const over = { id: view.id, after: event.clientY > box.top + box.height / 2 }
    if (drag.over?.id !== over.id || drag.over.after !== over.after) setDrag({ ...drag, over })
  }
  const drop = (event: DragEvent<HTMLElement>) => {
    if (!drag) return
    event.preventDefault()
    const order = drag.over ? orderAfterDrop(pinned, drag.id, drag.over) : null
    setDrag(null)
    if (order) void reorder(order)
  }

  return (
    <SidebarSection label="Views">
      {pinned.map((view) => (
        <li
          className={drag?.over?.id === view.id && drag.id !== view.id ? (drag.over.after ? 'aw-dropline is-after' : 'aw-dropline is-before') : undefined}
          key={view.id}
          onDragOver={(event) => dragOver(event, view)}
          onDrop={drop}
        >
          <NavLink
            className={() =>
              ['aw-item', view.id === current && location.pathname.endsWith('/work-items') ? 'is-active' : '', drag?.id === view.id ? 'is-dragging' : '']
                .filter(Boolean)
                .join(' ')
            }
            onClick={onNavigate}
            onDragEnd={() => setDrag(null)}
            onDragStart={(event) => {
              if (isMobileWidth()) return
              event.dataTransfer.effectAllowed = 'all'
              event.dataTransfer.setData(VIEW_DRAG_TYPE, view.id)
              setDrag({ id: view.id, over: null })
            }}
            to={taskViewHref(projectId, view)}
          >
            <span className="aw-item__lead">
              <Icon name={view.builtin === 'ready_for_agents' ? 'bot' : view.type === 'board' ? 'board' : 'list'} />
            </span>
            <span className="aw-item__label">{viewName(view, experience)}</span>
          </NavLink>
        </li>
      ))}
    </SidebarSection>
  )
}
