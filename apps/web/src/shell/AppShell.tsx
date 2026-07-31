import { useEffect, useRef, useState, type ReactNode } from 'react'

import type { SessionUser } from '../auth/session'
import type { Project } from '../projects/types'
import { ProjectRail } from './ProjectRail'
import { ProjectSidebar } from './ProjectSidebar'

export function AppShell({
  children,
  context,
  mainClassName = 'project-workspace',
  project = null,
  thread = null,
  user,
}: {
  children: ReactNode
  context?: ReactNode
  mainClassName?: string
  project?: Project | null
  thread?: { id: string; name: string } | null
  user: SessionUser
}) {
  const [railOpen, setRailOpen] = useState(false)
  const [sidebarOpen, setSidebarOpen] = useState(false)
  const railButtonRef = useRef<HTMLButtonElement>(null)
  const sidebarButtonRef = useRef<HTMLButtonElement>(null)
  const anyDrawerOpen = railOpen || sidebarOpen

  const closeDrawers = () => {
    setRailOpen(false)
    setSidebarOpen(false)
  }

  useEffect(() => {
    if (!anyDrawerOpen) return
    const closeOnEscape = (event: KeyboardEvent) => {
      if (event.key !== 'Escape') return
      const returnFocus = railOpen
        ? railButtonRef.current
        : sidebarButtonRef.current
      closeDrawers()
      window.requestAnimationFrame(() => returnFocus?.focus())
    }
    document.addEventListener('keydown', closeOnEscape)
    return () => document.removeEventListener('keydown', closeOnEscape)
  }, [anyDrawerOpen, railOpen])

  useEffect(() => {
    if (railOpen) {
      document
        .querySelector<HTMLElement>('#project-rail a, #project-rail button')
        ?.focus()
    } else if (sidebarOpen) {
      document
        .querySelector<HTMLElement>('#project-sidebar a, #project-sidebar button')
        ?.focus()
    }
  }, [railOpen, sidebarOpen])

  return (
    <div className="app-shell" data-has-project={Boolean(project)}>
      <a className="skip-link" href="#main-workspace">
        Skip to main workspace
      </a>
      <ProjectRail
        currentProjectId={project?.id ?? null}
        mobileOpen={railOpen}
        onNavigate={closeDrawers}
        user={user}
      />
      {project && (
        <ProjectSidebar
          mobileOpen={sidebarOpen}
          onNavigate={closeDrawers}
          project={project}
          thread={thread}
        />
      )}
      <main className={mainClassName} id="main-workspace" tabIndex={-1}>
        <div className="mobile-toolbar">
          <button
            aria-controls="project-rail"
            aria-expanded={railOpen}
            onClick={() => {
              setSidebarOpen(false)
              setRailOpen((open) => !open)
            }}
            ref={railButtonRef}
            type="button"
          >
            Projects
          </button>
          {project && (
            <button
              aria-controls="project-sidebar"
              aria-expanded={sidebarOpen}
              onClick={() => {
                setRailOpen(false)
                setSidebarOpen((open) => !open)
              }}
              ref={sidebarButtonRef}
              type="button"
            >
              {project.key} menu
            </button>
          )}
        </div>
        {children}
      </main>
      {project && context}
      {anyDrawerOpen && (
        <button
          aria-label="Close navigation"
          className="drawer-backdrop"
          onClick={() => {
            const returnFocus = railOpen
              ? railButtonRef.current
              : sidebarButtonRef.current
            closeDrawers()
            window.requestAnimationFrame(() => returnFocus?.focus())
          }}
          type="button"
        />
      )}
    </div>
  )
}
