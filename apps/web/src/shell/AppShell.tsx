import { useCallback, useEffect, useRef, useState, type ReactNode } from 'react'

import type { SessionUser } from '../auth/session'
import { DirectSidebar } from '../direct/DirectSidebar'
import type { Project } from '../projects/types'
import { useOptionalRealtime } from '../realtime/realtimeContext'
import { Banner, Icon } from '../ui'
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

/**
 * The app frame: the rail of spaces, the space's sidebar, the conversation or page,
 * and an optional details panel. On narrow screens each side column becomes a drawer.
 */
export function AppShell({
  children,
  context,
  direct = false,
  mainClassName,
  project = null,
  selectedChannelId,
  thread = null,
  user,
}: {
  children: ReactNode
  /** The details panel (thread details, group members). */
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

  const returnFocus = useCallback(
    () => (railOpen ? railButtonRef.current : sidebarOpen ? sidebarButtonRef.current : contextButtonRef.current),
    [railOpen, sidebarOpen],
  )

  useEffect(() => {
    if (!anyDrawerOpen) return
    const activeDrawer = shellRef.current?.querySelector<HTMLElement>(
      railOpen ? '#project-rail' : sidebarOpen ? '#project-sidebar' : '#context-region',
    )

    const handleDrawerKeyDown = (event: KeyboardEvent) => {
      if (event.key === 'Escape') {
        const trigger = returnFocus()
        closeDrawers()
        window.requestAnimationFrame(() => trigger?.focus())
        return
      }

      if (event.key !== 'Tab') return
      const drawerControls = Array.from(activeDrawer?.querySelectorAll<HTMLElement>(drawerFocusableSelector) ?? []).filter(
        (element) => element.tabIndex >= 0 && !element.closest('[inert], [aria-hidden="true"]'),
      )
      const focusable = closeButtonRef.current ? [...drawerControls, closeButtonRef.current] : drawerControls
      if (!focusable.length) return

      const currentIndex = focusable.indexOf(document.activeElement as HTMLElement)
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
    const sidebar = shellRef.current?.querySelector<HTMLElement>('#project-sidebar') ?? null
    const contextRegion = shellRef.current?.querySelector<HTMLElement>('#context-region') ?? null
    const contextDrawer = contextRegion?.querySelector<HTMLElement>('.context-drawer') ?? null

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
        document.querySelector<HTMLElement>('#project-rail a, #project-rail button')?.focus()
      } else if (sidebarOpen) {
        document.querySelector<HTMLElement>('#project-sidebar a, #project-sidebar button')?.focus()
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
      className="aw-shell"
      data-drawer={railOpen ? 'rail' : sidebarOpen ? 'sidebar' : contextOpen ? 'context' : undefined}
      data-has-context={Boolean(hasSidebar && context)}
      data-has-sidebar={hasSidebar}
      ref={shellRef}
    >
      <a className="aw-skip-link" href="#main-workspace" inert={anyDrawerOpen}>
        Skip to main workspace
      </a>
      <ProjectRail currentProjectId={project?.id ?? null} mobileOpen={railOpen} onNavigate={closeDrawers} user={user} />
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
        className={mainClassName ? `aw-main ${mainClassName}` : 'aw-main aw-main--page'}
        data-region="content"
        id="main-workspace"
        inert={anyDrawerOpen}
        tabIndex={-1}
      >
        <div className="aw-mobilebar">
          <button
            aria-controls="project-rail"
            aria-expanded={railOpen}
            className="aw-mobilebar__button"
            onClick={() => {
              setSidebarOpen(false)
              setContextOpen(false)
              setRailOpen((open) => !open)
            }}
            ref={railButtonRef}
            type="button"
          >
            <Icon name="menu" size={20} />
            <span className="visually-hidden">Projects</span>
          </button>
          {hasSidebar && (
            <button
              aria-controls="project-sidebar"
              aria-expanded={sidebarOpen}
              className="aw-mobilebar__button"
              onClick={() => {
                setRailOpen(false)
                setContextOpen(false)
                setSidebarOpen((open) => !open)
              }}
              ref={sidebarButtonRef}
              type="button"
            >
              <span aria-hidden="true" className="aw-mobilebar__space">
                {project ? project.name : 'Messages'}
                <Icon name="chevron-down" size={14} />
              </span>
              <span className="visually-hidden">{project ? `${project.key} menu` : 'Messages menu'}</span>
            </button>
          )}
          {hasSidebar && context && (
            <button
              aria-controls="context-region"
              aria-expanded={contextOpen}
              className="aw-mobilebar__button aw-mobilebar__details"
              onClick={() => {
                setRailOpen(false)
                setSidebarOpen(false)
                setContextOpen((open) => !open)
              }}
              ref={contextButtonRef}
              type="button"
            >
              <Icon name="panel-right" size={18} />
              Details
            </button>
          )}
        </div>
        {realtime?.connection === 'retrying' && (
          <Banner className="aw-shell__banner" title="Reconnecting." tone="offline">
            Live updates are paused for a moment. Anything you send still goes through.
          </Banner>
        )}
        {mainClassName ? children : <div className="aw-legacy">{children}</div>}
      </main>
      {hasSidebar && context && (
        <div
          aria-label={contextOpen ? 'Details' : undefined}
          aria-modal={contextOpen || undefined}
          className="aw-panel"
          data-mobile-open={contextOpen}
          data-region="content"
          id="context-region"
          role={contextOpen ? 'dialog' : undefined}
          tabIndex={-1}
        >
          {contextOpen && (
            <button
              aria-label="Close Details"
              autoFocus
              className="aw-iconbtn aw-panel__close"
              onClick={() => {
                setContextOpen(false)
                window.requestAnimationFrame(() => contextButtonRef.current?.focus())
              }}
              ref={contextCloseButtonRef}
              type="button"
            >
              <Icon name="x" size={18} />
            </button>
          )}
          {context}
        </div>
      )}
      {anyDrawerOpen && (
        <button
          aria-label={contextOpen ? 'Dismiss Details' : 'Close navigation'}
          className="aw-drawer-backdrop"
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
