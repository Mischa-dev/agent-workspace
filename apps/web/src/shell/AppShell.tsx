import { Suspense, lazy, useCallback, useEffect, useMemo, useRef, useState, type ReactNode } from 'react'
import { useLocation } from 'react-router-dom'

import type { SessionUser } from '../auth/session'
import { useProjects } from '../projects/hooks'
import type { Project } from '../projects/types'
import { useOptionalRealtime } from '../realtime/realtimeContext'
import { syncThemeColor } from '../theme/themeColor'
import { Banner, Icon } from '../ui'
import { staticPageTitle } from './documentTitleModel'
import { loadShellLazy } from './loadShellLazy'
import { MenuButton } from './MenuButton'
import { MOBILE_QUERY, ShellHeaderContext, isMobileWidth, useMobileNavigation } from './mobileNavigation'
import { ProjectRail } from './ProjectRail'
import { ProjectSidebar } from './ProjectSidebar'
import { useKeyboardViewport } from './useKeyboardViewport'
import { useSwipe } from './useSwipe'

const drawerFocusableSelector = [
  'a[href]',
  'button:not([disabled])',
  'input:not([disabled])',
  'select:not([disabled])',
  'textarea:not([disabled])',
  '[tabindex]:not([tabindex="-1"])',
].join(', ')

const PullToRefresh = lazy(() => loadShellLazy().then((module) => ({ default: module.PullToRefresh })))
const HomeSidebar = lazy(() => loadShellLazy().then((module) => ({ default: module.HomeSidebar })))
const DirectSidebar = lazy(() => loadShellLazy().then((module) => ({ default: module.DirectSidebar })))

/**
 * The listed project a /projects/… address belongs to, so a project page
 * that's still loading (or failed) keeps its sidebar instead of jumping.
 */
function useAddressProject(enabled: boolean): Project | null {
  const location = useLocation()
  const projects = useProjects()
  if (!enabled) return null
  const match = /^\/projects\/([^/]+)/.exec(location.pathname)
  if (!match || match[1] === 'new') return null
  const id = decodeURIComponent(match[1])
  return projects.data?.projects.find((candidate) => candidate.id === id && !candidate.archivedAt) ?? null
}

/** A home-screen app on a touch screen, where there's no reload button. */
function isInstalledTouchApp() {
  if (typeof window.matchMedia !== 'function') return false
  const standalone = window.matchMedia('(display-mode: standalone)').matches || (navigator as Navigator & { standalone?: boolean }).standalone === true
  return standalone && window.matchMedia('(pointer: coarse)').matches
}

function setInert(element: HTMLElement | null, inert: boolean) {
  if (inert) {
    element?.setAttribute('inert', '')
  } else {
    element?.removeAttribute('inert')
  }
}

/**
 * The app frame: the rail of spaces, the space's sidebar, the conversation or page,
 * and an optional details panel. On narrow screens the rail and sidebar become one
 * navigation drawer, and the details panel becomes a drawer on the right.
 */
export function AppShell({
  children,
  context,
  contextKey,
  direct = false,
  home = false,
  mainClassName,
  onContextDismiss,
  project: givenProject = null,
  selectedChannelId,
  thread = null,
  user,
}: {
  children: ReactNode
  /** The details panel (thread details, group members). */
  context?: ReactNode
  /**
   * Changes when the person asks to see something in the panel, such as a
   * thread. On narrow screens the panel is a drawer, so it opens then.
   */
  contextKey?: string
  /** Show the Messages sidebar (DMs and groups) instead of a project's. */
  direct?: boolean
  /** Show the Home sidebar (Inbox, My work, Later, Runs, Whiteboards). */
  home?: boolean
  mainClassName?: string
  /**
   * Called when the person closes the details drawer on a narrow screen, so the
   * page can forget the panel and its own toggle opens it again in one tap.
   */
  onContextDismiss?: () => void
  project?: Project | null
  selectedChannelId?: string
  thread?: { id: string; name: string } | null
  user: SessionUser
}) {
  useKeyboardViewport()
  const location = useLocation()
  const addressProject = useAddressProject(!givenProject && !home && !direct)
  const project = givenProject ?? addressProject
  const navigation = useMobileNavigation()
  const navOpen = navigation.open
  const setNavOpen = navigation.setOpen
  const [contextOpenState, setContextOpen] = useState(false)
  // A panel closed from inside (a thread's close button) closes its drawer too.
  const contextOpen = contextOpenState && Boolean(context)
  const shellRef = useRef<HTMLDivElement>(null)
  const mainRef = useRef<HTMLElement>(null)
  const navTriggerRef = useRef<HTMLElement | null>(null)
  const contextCloseButtonRef = useRef<HTMLButtonElement>(null)
  const closeButtonRef = useRef<HTMLButtonElement>(null)
  const contextTriggerRef = useRef<HTMLElement | null>(null)
  const anyDrawerOpen = navOpen || contextOpen
  // Conversations bring their own header, which carries the menu button on phones.
  const conversationLayout = mainClassName === 'aw-main--conversation'
  const [installedTouchApp] = useState(isInstalledTouchApp)

  // When the page asks to show something new in the panel (a thread), open
  // the drawer on narrow screens. Adjusting state while rendering, not in an
  // effect, avoids a second render pass.
  const [shownContextKey, setShownContextKey] = useState<string | undefined>(undefined)
  if (contextKey !== shownContextKey) {
    setShownContextKey(contextKey)
    if (contextKey && isMobileWidth()) {
      if (navOpen) setNavOpen(false)
      setContextOpen(true)
    }
  }
  const realtime = useOptionalRealtime()
  const hasSidebar = Boolean(project) || direct || home

  const closeDrawers = useCallback(() => {
    setNavOpen(false)
    setContextOpen(false)
  }, [setNavOpen])

  const dismiss = useCallback(() => {
    const trigger = navOpen ? navTriggerRef.current : contextTriggerRef.current
    if (contextOpen) onContextDismiss?.()
    closeDrawers()
    window.requestAnimationFrame(() => trigger?.focus())
  }, [closeDrawers, contextOpen, navOpen, onContextDismiss])

  const openNavigation = useCallback(
    (trigger: HTMLElement | null) => {
      navTriggerRef.current = trigger
      setContextOpen(false)
      setNavOpen(true)
    },
    [setNavOpen],
  )

  const shellHeader = useMemo(() => ({ openNavigation, navigationOpen: navOpen }), [navOpen, openNavigation])

  // The details panel's trigger lives in the page header; remember what had
  // focus when it opened (before focus moves into the drawer, below) so
  // closing the drawer can return there.
  useEffect(() => {
    if (contextOpen) contextTriggerRef.current = document.activeElement instanceof HTMLElement ? document.activeElement : null
  }, [contextOpen])

  // Widening past the phone layout shows everything in place, so drop the drawers.
  useEffect(() => {
    if (!anyDrawerOpen || typeof window.matchMedia !== 'function') return undefined
    const query = window.matchMedia(MOBILE_QUERY)
    const onChange = () => {
      if (!query.matches) closeDrawers()
    }
    query.addEventListener?.('change', onChange)
    return () => query.removeEventListener?.('change', onChange)
  }, [anyDrawerOpen, closeDrawers])

  useEffect(() => {
    const root = document.documentElement
    root.dataset.chrome = conversationLayout ? 'conversation' : 'page'
    syncThemeColor()
  }, [conversationLayout])

  useEffect(() => {
    if (!anyDrawerOpen) return
    const drawers = Array.from(
      shellRef.current?.querySelectorAll<HTMLElement>(navOpen ? '#project-rail, #project-sidebar' : '#context-region') ?? [],
    )

    const handleDrawerKeyDown = (event: KeyboardEvent) => {
      if (event.key === 'Escape') {
        dismiss()
        return
      }

      if (event.key !== 'Tab') return
      const drawerControls = drawers
        .flatMap((drawer) => Array.from(drawer.querySelectorAll<HTMLElement>(drawerFocusableSelector)))
        .filter((element) => element.tabIndex >= 0 && !element.closest('[inert], [aria-hidden="true"]'))
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
  }, [anyDrawerOpen, dismiss, navOpen])

  useEffect(() => {
    const rail = shellRef.current?.querySelector<HTMLElement>('#project-rail') ?? null
    const sidebar = shellRef.current?.querySelector<HTMLElement>('#project-sidebar') ?? null
    const contextRegion = shellRef.current?.querySelector<HTMLElement>('#context-region') ?? null
    const contextDrawer = contextRegion?.querySelector<HTMLElement>('.context-drawer') ?? null

    setInert(rail, anyDrawerOpen && !navOpen)
    setInert(sidebar, anyDrawerOpen && !navOpen)
    setInert(contextRegion, anyDrawerOpen && !contextOpen)
    setInert(contextDrawer, anyDrawerOpen && !contextOpen)

    return () => {
      setInert(rail, false)
      setInert(sidebar, false)
      setInert(contextRegion, false)
      setInert(contextDrawer, false)
    }
  }, [anyDrawerOpen, contextOpen, navOpen])

  useEffect(() => {
    if (!navOpen && !contextOpen) return
    const focusTimer = window.setTimeout(() => {
      if (navOpen) {
        // Start where you are: the current channel or conversation, else the first destination.
        const target =
          document.querySelector<HTMLElement>('#project-sidebar [aria-current="page"]') ??
          document.querySelector<HTMLElement>('#project-sidebar a') ??
          document.querySelector<HTMLElement>('#project-rail a, #project-rail button')
        target?.focus()
      } else if (contextOpen) {
        const contextRegion = document.querySelector<HTMLElement>('#context-region')
        const focusTarget = contextCloseButtonRef.current ?? contextRegion
        focusTarget?.focus()
      }
    }, 50)
    return () => window.clearTimeout(focusTimer)
  }, [contextOpen, navOpen])

  useSwipe(shellRef, {
    onSwipeLeft: () => {
      if (navOpen) dismiss()
    },
    onSwipeRight: (target) => {
      if (contextOpen) dismiss()
      else if (!navOpen && isMobileWidth() && target.closest('#main-workspace')) openNavigation(null)
    },
  })

  return (
    <ShellHeaderContext.Provider value={shellHeader}>
      <div
        className="aw-shell"
        data-drawer={navOpen ? 'nav' : contextOpen ? 'context' : undefined}
        data-has-context={Boolean(hasSidebar && context)}
        data-has-sidebar={hasSidebar}
        ref={shellRef}
      >
        <a className="aw-skip-link" href="#main-workspace" inert={anyDrawerOpen}>
          Skip to main workspace
        </a>
        <ProjectRail
          currentProjectId={project?.id ?? null}
          mobileOpen={navOpen}
          onNavigate={closeDrawers}
          onSpaceSelected={navigation.keepOpenOnNextNavigation}
          user={user}
        />
        {project ? (
          <ProjectSidebar
            mobileOpen={navOpen}
            onNavigate={closeDrawers}
            project={project}
            selectedChannelId={selectedChannelId}
            thread={thread}
          />
        ) : direct || home ? (
          // These load just after the first paint; the empty column holds the layout still.
          <Suspense fallback={<div className="aw-sidebar" />}>
            {direct ? (
              <DirectSidebar mobileOpen={navOpen} onNavigate={closeDrawers} />
            ) : (
              <HomeSidebar mobileOpen={navOpen} onNavigate={closeDrawers} />
            )}
          </Suspense>
        ) : navOpen ? (
          // A page with no sidebar of its own (Search, Settings) still gets a
          // useful drawer on a phone: Home's places beside the rail.
          <div className="aw-shell__drawer-sidebar">
            <Suspense fallback={null}>
              <HomeSidebar mobileOpen={navOpen} onNavigate={closeDrawers} />
            </Suspense>
          </div>
        ) : null}
        <main
          className={`aw-main ${mainClassName ?? 'aw-main--page'}`}
          data-region="content"
          id="main-workspace"
          inert={anyDrawerOpen}
          ref={mainRef}
          tabIndex={-1}
        >
          {installedTouchApp ? (
            <Suspense fallback={null}>
              {/* Conversations scroll up for older messages, so pulling there means that. */}
              <PullToRefresh enabled={!conversationLayout && !anyDrawerOpen} target={mainRef} />
            </Suspense>
          ) : null}
          {conversationLayout ? null : (
            <div className="aw-mobilebar">
              <MenuButton label={project ? project.name : direct ? 'Messages' : staticPageTitle(location.pathname) || (home ? 'Home' : 'Agent Workspace')} />
            </div>
          )}
          {realtime?.connection === 'retrying' && (
            <Banner className="aw-shell__banner" title="Reconnecting." tone="offline">
              Live updates are paused for a moment. Anything you send still goes through.
            </Banner>
          )}
          {children}
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
                aria-label="Close details"
                className="aw-iconbtn aw-panel__close"
                onClick={dismiss}
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
            aria-label={contextOpen ? 'Dismiss details' : 'Close navigation'}
            className="aw-drawer-backdrop"
            onClick={dismiss}
            ref={closeButtonRef}
            type="button"
          />
        )}
      </div>
    </ShellHeaderContext.Provider>
  )
}
