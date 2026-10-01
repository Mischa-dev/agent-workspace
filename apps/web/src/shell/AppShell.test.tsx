import { useState } from 'react'
import { act, cleanup, fireEvent, render, screen, waitFor } from '@testing-library/react'
import { MemoryRouter, useNavigate } from 'react-router-dom'
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'

import type { SessionUser } from '../auth/session'
import type { Project } from '../projects/types'
import { AppShell } from './AppShell'
import { MobileNavigationProvider } from './MobileNavigationProvider'
import { useMobileNavigation } from './mobileNavigation'

vi.mock('./ProjectRail', () => ({
  ProjectRail: ({ mobileOpen, onSpaceSelected }: { mobileOpen: boolean; onSpaceSelected: () => void }) => (
    <nav data-mobile-open={mobileOpen} id="project-rail">
      <a href="/home">First rail destination</a>
      <button onClick={onSpaceSelected} type="button">
        Last rail action
      </button>
    </nav>
  ),
}))

vi.mock('./ProjectSidebar', () => ({
  ProjectSidebar: ({ mobileOpen }: { mobileOpen: boolean }) => (
    <aside data-mobile-open={mobileOpen} id="project-sidebar">
      <a href="/projects/project-a">First sidebar destination</a>
      <button type="button">Last sidebar action</button>
    </aside>
  ),
}))

const user: SessionUser = {
  id: 'user-a',
  actorId: 'actor-a',
  actorKind: 'human',
  loginName: 'mischa',
  displayName: 'Mischa Nelson',
  capabilities: { createProject: true },
}

const project: Project = {
  id: 'project-a',
  organizationId: 'organization-a',
  key: 'AW',
  name: 'Agent Workspace',
  archivedAt: null,
  role: 'owner',
  version: 1,
  attentionCount: 0,
  capabilities: {
    view: true,
    manage: true,
    archive: true,
    navigationWrite: true,
  },
  navigation: {
    location: { kind: 'project_overview' },
    href: '/projects/project-a',
    pinned: true,
    lastOpenedAt: '2026-07-31T08:00:00Z',
    version: 1,
  },
}

describe('AppShell on narrow screens', () => {
  beforeEach(() => {
    vi.stubGlobal('requestAnimationFrame', (callback: FrameRequestCallback) => {
      callback(0)
      return 1
    })
    vi.stubGlobal('matchMedia', (query: string) => ({
      matches: query.includes('max-width'),
      addEventListener: vi.fn(),
      removeEventListener: vi.fn(),
    }))
  })

  afterEach(() => {
    cleanup()
    vi.unstubAllGlobals()
  })

  it('opens the rail and sidebar as one drawer, keeps focus inside it, and makes the rest inert', async () => {
    render(
      <AppShell context={<aside className="context-drawer">Context</aside>} project={project} user={user}>
        <button type="button">Main action</button>
      </AppShell>,
    )

    const menuButton = screen.getByRole('button', { name: 'Agent Workspace: open navigation' })
    const rail = document.querySelector<HTMLElement>('#project-rail')
    const sidebar = document.querySelector<HTMLElement>('#project-sidebar')
    const main = document.querySelector<HTMLElement>('#main-workspace')
    const context = document.querySelector<HTMLElement>('.context-drawer')
    const skipLink = screen.getByRole('link', { name: 'Skip to main workspace' })

    fireEvent.click(menuButton)
    expect(menuButton).toHaveAttribute('aria-expanded', 'true')
    expect(rail).toHaveAttribute('data-mobile-open', 'true')
    expect(sidebar).toHaveAttribute('data-mobile-open', 'true')
    expect(rail).not.toHaveAttribute('inert')
    expect(sidebar).not.toHaveAttribute('inert')
    expect(main).toHaveAttribute('inert')
    expect(context).toHaveAttribute('inert')
    expect(skipLink).toHaveAttribute('inert')

    const closeButton = screen.getByRole('button', { name: 'Close navigation' })
    const firstRail = screen.getByRole('link', { name: 'First rail destination' })
    const firstSidebar = screen.getByRole('link', { name: 'First sidebar destination' })
    const lastSidebar = screen.getByRole('button', { name: 'Last sidebar action' })
    await waitFor(() => expect(firstSidebar).toHaveFocus())

    fireEvent.keyDown(document, { key: 'Tab' })
    expect(lastSidebar).toHaveFocus()
    fireEvent.keyDown(document, { key: 'Tab' })
    expect(closeButton).toHaveFocus()
    fireEvent.keyDown(document, { key: 'Tab' })
    expect(firstRail).toHaveFocus()
    fireEvent.keyDown(document, { key: 'Tab', shiftKey: true })
    expect(closeButton).toHaveFocus()

    fireEvent.keyDown(document, { key: 'Escape' })
    expect(menuButton).toHaveAttribute('aria-expanded', 'false')
    expect(screen.queryByRole('button', { name: 'Close navigation' })).not.toBeInTheDocument()
    expect(menuButton).toHaveFocus()
    expect(main).not.toHaveAttribute('inert')
    expect(skipLink).not.toHaveAttribute('inert')
  })

  it('closes the drawer from the backdrop and restores its trigger focus', () => {
    render(
      <AppShell project={project} user={user}>
        <h1>Workspace</h1>
      </AppShell>,
    )

    const menuButton = screen.getByRole('button', { name: 'Agent Workspace: open navigation' })
    fireEvent.click(menuButton)
    fireEvent.click(screen.getByRole('button', { name: 'Close navigation' }))
    expect(menuButton).toHaveAttribute('aria-expanded', 'false')
    expect(menuButton).toHaveFocus()
  })

  it('opens details as a modal drawer and tells the page when it is dismissed', async () => {
    function Page() {
      const [open, setOpen] = useState(false)
      return (
        <AppShell
          context={open ? <aside className="context-drawer">Thread details</aside> : undefined}
          contextKey={open ? 'details' : undefined}
          mainClassName="aw-main--conversation"
          onContextDismiss={() => setOpen(false)}
          project={project}
          user={user}
        >
          <button aria-pressed={open} onClick={() => setOpen(!open)} type="button">
            Show details
          </button>
        </AppShell>
      )
    }
    render(<Page />)

    // A conversation brings its own header, so the shell's bar is gone.
    expect(screen.queryByRole('button', { name: /open navigation/ })).not.toBeInTheDocument()

    const toggle = screen.getByRole('button', { name: 'Show details' })
    toggle.focus()
    fireEvent.click(toggle)
    expect(screen.getByRole('dialog', { name: 'Details' })).toHaveAttribute('aria-modal', 'true')
    expect(document.querySelector('#main-workspace')).toHaveAttribute('inert')
    await waitFor(() => expect(screen.getByRole('button', { name: 'Close Details' })).toHaveFocus())

    fireEvent.keyDown(document, { key: 'Escape' })
    expect(screen.queryByRole('dialog')).not.toBeInTheDocument()
    expect(toggle).toHaveAttribute('aria-pressed', 'false')
    expect(toggle).toHaveFocus()

    // Dismissing reset the page, so its toggle opens the drawer again in one tap.
    fireEvent.click(toggle)
    expect(screen.getByRole('dialog', { name: 'Details' })).toBeInTheDocument()
  })
})

describe('MobileNavigationProvider', () => {
  function Probe() {
    const navigation = useMobileNavigation()
    const navigate = useNavigate()
    return (
      <>
        <output aria-label="Drawer">{navigation.open ? 'open' : 'closed'}</output>
        <button onClick={() => navigation.setOpen(true)} type="button">
          Open
        </button>
        <button
          onClick={() => {
            navigation.keepOpenOnNextNavigation()
            navigate('/projects/b')
          }}
          type="button"
        >
          Switch Project
        </button>
        <button onClick={() => navigate('/projects/b/channels/c')} type="button">
          Open channel
        </button>
      </>
    )
  }

  it('keeps the drawer open through a space switch and closes it on any other navigation', () => {
    render(
      <MemoryRouter initialEntries={['/projects/a']}>
        <MobileNavigationProvider>
          <Probe />
        </MobileNavigationProvider>
      </MemoryRouter>,
    )
    const drawer = screen.getByRole('status', { name: 'Drawer' })

    fireEvent.click(screen.getByRole('button', { name: 'Open' }))
    act(() => fireEvent.click(screen.getByRole('button', { name: 'Switch Project' })))
    expect(drawer).toHaveTextContent('open')

    act(() => fireEvent.click(screen.getByRole('button', { name: 'Open channel' })))
    expect(drawer).toHaveTextContent('closed')
  })
})
