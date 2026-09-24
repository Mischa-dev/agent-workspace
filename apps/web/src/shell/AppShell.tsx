import { useCallback, useEffect, useRef, useState, type ReactNode } from 'react'

import type { SessionUser } from '../auth/session'
import { DirectSidebar } from '../direct/DirectSidebar'
import type { Project } from '../projects/types'
import { useOptionalRealtime } from '../realtime/realtimeContext'
import { ProjectRail } from './ProjectRail'
import { ProjectSidebar } from './ProjectSidebar'

const drawerFocusableSelector = [
  'a[href]',
  'button:not([disabled])',
  'input:not([disabled])',
  'select:not([disabled])',
  'textarea:not([disabled])',
  '[tabindex]:not([tabindex="-1"])',
].join(', ')

function setInert(element: HTMLElement | null, inert: boolean) {
  if (inert) {
    element?.setAttribute('inert', '')
  } else {
    element?.removeAttribute('inert')
  }
}

export function AppShell({
  children,
  context,
  direct = false,
  mainClassName = 'project-workspace',
  project = null,
  selectedChannelId,
  thread = null,
  user,
}: {
  children: ReactNode
  context?: ReactNode
  /** Show the Messages sidebar (DMs and groups) instead of a Project's. */
  direct?: boolean
  mainClassName?: string
  project?: Project | null
  selectedChannelId?: string
  thread?: { id: string; name: string } | null
  user: SessionUser
}) {
  const [railOpen, setRailOpen] = useState(false)
  const [sidebarOpen, setSidebarOpen] = useState(false)
  const [contextOpen, setContextOpen] = useState(false)
  const shellRef = useRef<HTMLDivElement>(null)
  const railButtonRef = useRef<HTMLButtonElement>(null)
  const sidebarButtonRef = useRef<HTMLButtonElement>(null)
  const contextButtonRef = useRef<HTMLButtonElement>(null)
  const contextCloseButtonRef = useRef<HTMLButtonElement>(null)
  const closeButtonRef = useRef<HTMLButtonElement>(null)
  const anyDrawerOpen = railOpen || sidebarOpen || contextOpen
  const realtime = useOptionalRealtime()
  const hasSidebar = Boolean(project) || direct

  const closeDrawers = () => {
    setRailOpen(false)
    setSidebarOpen(false)
    setContextOpen(false)
  }

  const returnFocus = useCallback(() =>
    railOpen
      ? railButtonRef.current
      : sidebarOpen
        ? sidebarButtonRef.current
        : contextButtonRef.current
  , [railOpen, sidebarOpen])

  useEffect(() => {
    if (!anyDrawerOpen) return
    const activeDrawer = shellRef.current?.querySelector<HTMLElement>(
      railOpen
        ? '#project-rail'
        : sidebarOpen
          ? '#project-sidebar'
          : '#context-region',
    )

    const handleDrawerKeyDown = (event: KeyboardEvent) => {
      if (event.key === 'Escape') {
        const trigger = returnFocus()
        closeDrawers()
        window.requestAnimationFrame(() => trigger?.focus())
        return
      }

      if (event.key !== 'Tab') return
      const drawerControls = Array.from(
        activeDrawer?.querySelectorAll<HTMLElement>(drawerFocusableSelector) ??
          [],
      ).filter(
        (element) =>
          element.tabIndex >= 0 && !element.closest('[inert], [aria-hidden="true"]'),
      )
      const focusable = closeButtonRef.current
        ? [...drawerControls, closeButtonRef.current]
        : drawerControls
      if (!focusable.length) return

      const currentIndex = focusable.indexOf(
        document.activeElement as HTMLElement,
      )
      const nextIndex = event.shiftKey
        ? currentIndex <= 0
          ? focusable.length - 1
          : currentIndex - 1
        : currentIndex < 0 || currentIndex === focusable.length - 1
          ? 0
          : currentIndex + 1

      event.preventDefault()
      focusable[nextIndex]?.focus()
    }
    document.addEventListener('keydown', handleDrawerKeyDown)
    return () => document.removeEventListener('keydown', handleDrawerKeyDown)
  }, [anyDrawerOpen, contextOpen, railOpen, returnFocus, sidebarOpen])

  useEffect(() => {
    const rail = shellRef.current?.querySelector<HTMLElement>('#project-rail') ?? null
    const sidebar =
      shellRef.current?.querySelector<HTMLElement>('#project-sidebar') ?? null
    const contextRegion =
      shellRef.current?.querySelector<HTMLElement>('#context-region') ?? null
    const contextDrawer =
      contextRegion?.querySelector<HTMLElement>('.context-drawer') ?? null

    setInert(rail, anyDrawerOpen && !railOpen)
    setInert(sidebar, anyDrawerOpen && !sidebarOpen)
    setInert(contextRegion, anyDrawerOpen && !contextOpen)
    setInert(contextDrawer, anyDrawerOpen && !contextOpen)

    return () => {
      setInert(rail, false)
      setInert(sidebar, false)
      setInert(contextRegion, false)
      setInert(contextDrawer, false)
    }
  }, [anyDrawerOpen, contextOpen, railOpen, sidebarOpen])

  useEffect(() => {
    if (!railOpen && !sidebarOpen && !contextOpen) return
    const focusTimer = window.setTimeout(() => {
      if (railOpen) {
        document
          .querySelector<HTMLElement>('#project-rail a, #project-rail button')
          ?.focus()
      } else if (sidebarOpen) {
        document
          .querySelector<HTMLElement>('#project-sidebar a, #project-sidebar button')
          ?.focus()
      } else if (contextOpen) {
        const contextRegion = document.querySelector<HTMLElement>('#context-region')
        const focusTarget = contextCloseButtonRef.current ?? contextRegion
        focusTarget?.focus()
      }
    }, 50)
    return () => window.clearTimeout(focusTimer)
  }, [contextOpen, railOpen, sidebarOpen])

  return (
    <div
      className="app-shell"
      data-has-project={hasSidebar}
      ref={shellRef}
    >
      <a
        className="skip-link"
        href="#main-workspace"
        inert={anyDrawerOpen}
      >
        Skip to main workspace
      </a>
      <ProjectRail
        currentProjectId={project?.id ?? null}
        mobileOpen={railOpen}
        onNavigate={closeDrawers}
        user={user}
      />
      {project ? (
        <ProjectSidebar
          mobileOpen={sidebarOpen}
          onNavigate={closeDrawers}
          project={project}
          selectedChannelId={selectedChannelId}
          thread={thread}
        />
      ) : direct ? (
        <DirectSidebar mobileOpen={sidebarOpen} onNavigate={closeDrawers} />
      ) : null}
      <main
        className={mainClassName}
        id="main-workspace"
        inert={anyDrawerOpen}
        tabIndex={-1}
      >
        <div className="mobile-toolbar" data-has-context={Boolean(context)}>
          <button
            aria-controls="project-rail"
            aria-expanded={railOpen}
            onClick={() => {
              setSidebarOpen(false)
              setContextOpen(false)
              setRailOpen((open) => !open)
            }}
            ref={railButtonRef}
            type="button"
          >
            Projects
          </button>
          {hasSidebar && (
            <button
              aria-controls="project-sidebar"
              aria-expanded={sidebarOpen}
              onClick={() => {
                setRailOpen(false)
                setContextOpen(false)
                setSidebarOpen((open) => !open)
              }}
              ref={sidebarButtonRef}
              type="button"
            >
              {project ? `${project.key} menu` : 'Messages menu'}
            </button>
          )}
          {hasSidebar && context && (
            <button
              aria-controls="context-region"
              aria-expanded={contextOpen}
              className="details-trigger"
              onClick={() => {
                setRailOpen(false)
                setSidebarOpen(false)
                setContextOpen((open) => !open)
              }}
              ref={contextButtonRef}
              type="button"
            >
              Details
            </button>
          )}
        </div>
        {realtime?.connection === 'retrying' && (
          <div className="shell-connection-banner" role="status">
            <span aria-hidden="true">↻</span>
            Live updates are reconnecting. Durable workspace state is being rechecked.
          </div>
        )}
        {children}
      </main>
      {hasSidebar && context && (
        <div
          className="context-region"
          data-mobile-open={contextOpen}
          id="context-region"
          role={contextOpen ? 'dialog' : undefined}
          aria-label={contextOpen ? 'Details' : undefined}
          aria-modal={contextOpen || undefined}
          tabIndex={-1}
        >
          {contextOpen && (
            <button
              aria-label="Close Details"
              autoFocus
              className="context-close-button"
              onClick={() => {
                setContextOpen(false)
                window.requestAnimationFrame(() => contextButtonRef.current?.focus())
              }}
              ref={contextCloseButtonRef}
              type="button"
            >
              ×
            </button>
          )}
          {context}
        </div>
      )}
      {anyDrawerOpen && (
        <button
          aria-label={contextOpen ? 'Dismiss Details' : 'Close navigation'}
          className="drawer-backdrop"
          onClick={() => {
            const trigger = returnFocus()
            closeDrawers()
            window.requestAnimationFrame(() => trigger?.focus())
          }}
          ref={closeButtonRef}
          type="button"
        />
      )}
    </div>
  )
}
