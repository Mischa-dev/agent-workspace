import { useEffect, useRef, useState, type ReactNode } from 'react'

import type { SessionUser } from '../auth/session'
import type { Project } from '../projects/types'
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
  const shellRef = useRef<HTMLDivElement>(null)
  const railButtonRef = useRef<HTMLButtonElement>(null)
  const sidebarButtonRef = useRef<HTMLButtonElement>(null)
  const closeButtonRef = useRef<HTMLButtonElement>(null)
  const anyDrawerOpen = railOpen || sidebarOpen

  const closeDrawers = () => {
    setRailOpen(false)
    setSidebarOpen(false)
  }

  useEffect(() => {
    if (!anyDrawerOpen) return
    const activeDrawer = shellRef.current?.querySelector<HTMLElement>(
      railOpen ? '#project-rail' : '#project-sidebar',
    )

    const handleDrawerKeyDown = (event: KeyboardEvent) => {
      const modalDialog = shellRef.current?.querySelector<HTMLElement>(
        '[role="dialog"][aria-modal="true"]',
      )
      if (modalDialog) return

      if (event.key === 'Escape') {
        const returnFocus = railOpen
          ? railButtonRef.current
          : sidebarButtonRef.current
        closeDrawers()
        window.requestAnimationFrame(() => returnFocus?.focus())
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
  }, [anyDrawerOpen, railOpen])

  useEffect(() => {
    const rail = shellRef.current?.querySelector<HTMLElement>('#project-rail') ?? null
    const sidebar =
      shellRef.current?.querySelector<HTMLElement>('#project-sidebar') ?? null
    const contextDrawers = Array.from(
      shellRef.current?.querySelectorAll<HTMLElement>(':scope > .context-drawer') ??
        [],
    )

    setInert(rail, anyDrawerOpen && !railOpen)
    setInert(sidebar, anyDrawerOpen && !sidebarOpen)
    contextDrawers.forEach((drawer) => setInert(drawer, anyDrawerOpen))

    return () => {
      setInert(rail, false)
      setInert(sidebar, false)
      contextDrawers.forEach((drawer) => setInert(drawer, false))
    }
  }, [anyDrawerOpen, railOpen, sidebarOpen])

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
    <div
      className="app-shell"
      data-has-project={Boolean(project)}
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
      {project && (
        <ProjectSidebar
          mobileOpen={sidebarOpen}
          onNavigate={closeDrawers}
          project={project}
          thread={thread}
        />
      )}
      <main
        className={mainClassName}
        id="main-workspace"
        inert={anyDrawerOpen}
        tabIndex={-1}
      >
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
          ref={closeButtonRef}
          type="button"
        />
      )}
    </div>
  )
}
