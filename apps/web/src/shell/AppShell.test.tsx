import { cleanup, fireEvent, render, screen, waitFor } from '@testing-library/react'
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'

import type { SessionUser } from '../auth/session'
import type { Project } from '../projects/types'
import { AppShell } from './AppShell'

vi.mock('./ProjectRail', () => ({
  ProjectRail: ({
    mobileOpen,
  }: {
    mobileOpen: boolean
  }) => (
    <nav data-mobile-open={mobileOpen} id="project-rail">
      <a href="/home">First rail destination</a>
      <button type="button">Last rail action</button>
    </nav>
  ),
}))

vi.mock('./ProjectSidebar', () => ({
  ProjectSidebar: ({
    mobileOpen,
  }: {
    mobileOpen: boolean
  }) => (
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

const originalInnerWidth = window.innerWidth

describe('AppShell responsive drawers', () => {
  beforeEach(() => {
    Object.defineProperty(window, 'innerWidth', {
      configurable: true,
      value: 480,
    })
    vi.stubGlobal('requestAnimationFrame', (callback: FrameRequestCallback) => {
      callback(0)
      return 1
    })
  })

  afterEach(() => {
    cleanup()
    Object.defineProperty(window, 'innerWidth', {
      configurable: true,
      value: originalInnerWidth,
    })
    vi.unstubAllGlobals()
  })

  it('contains focus within the active drawer and makes the background and inactive drawer inert', async () => {
    render(
      <AppShell
        context={
          <aside aria-label="Context" className="context-drawer">
            <button type="button">Context action</button>
          </aside>
        }
        project={project}
        user={user}
      >
        <button type="button">Main action</button>
      </AppShell>,
    )

    const projectsButton = screen.getByRole('button', { name: 'Projects' })
    const sidebarButton = screen.getByRole('button', { name: 'AW menu' })
    const rail = document.querySelector<HTMLElement>('#project-rail')
    const sidebar = document.querySelector<HTMLElement>('#project-sidebar')
    const main = document.querySelector<HTMLElement>('#main-workspace')
    const context = document.querySelector<HTMLElement>('.context-drawer')
    const skipLink = screen.getByRole('link', {
      name: 'Skip to main workspace',
    })

    expect(rail).not.toHaveAttribute('inert')
    expect(sidebar).not.toHaveAttribute('inert')
    expect(main).not.toHaveAttribute('inert')

    fireEvent.click(projectsButton)
    expect(projectsButton).toHaveAttribute('aria-expanded', 'true')
    expect(sidebarButton).toHaveAttribute('aria-expanded', 'false')
    expect(rail).toHaveAttribute('data-mobile-open', 'true')
    expect(sidebar).toHaveAttribute('data-mobile-open', 'false')
    expect(rail).not.toHaveAttribute('inert')
    expect(sidebar).toHaveAttribute('inert')
    expect(main).toHaveAttribute('inert')
    expect(context).toHaveAttribute('inert')
    expect(skipLink).toHaveAttribute('inert')

    const closeButton = screen.getByRole('button', {
      name: 'Close navigation',
    })
    const firstRailControl = screen.getByRole('link', {
      name: 'First rail destination',
    })
    const lastRailControl = screen.getByRole('button', {
      name: 'Last rail action',
    })

    expect(closeButton).toBeVisible()
    await waitFor(() =>
      expect(firstRailControl).toHaveFocus(),
    )

    fireEvent.keyDown(document, { key: 'Tab' })
    expect(lastRailControl).toHaveFocus()
    fireEvent.keyDown(document, { key: 'Tab' })
    expect(closeButton).toHaveFocus()
    fireEvent.keyDown(document, { key: 'Tab' })
    expect(firstRailControl).toHaveFocus()
    fireEvent.keyDown(document, { key: 'Tab', shiftKey: true })
    expect(closeButton).toHaveFocus()
    fireEvent.keyDown(document, { key: 'Tab', shiftKey: true })
    expect(lastRailControl).toHaveFocus()

    screen.getByRole('button', { name: 'Main action' }).focus()
    expect(screen.getByRole('button', { name: 'Main action' })).toHaveFocus()
    fireEvent.keyDown(document, { key: 'Tab' })
    expect(firstRailControl).toHaveFocus()

    fireEvent.click(sidebarButton)
    expect(projectsButton).toHaveAttribute('aria-expanded', 'false')
    expect(sidebarButton).toHaveAttribute('aria-expanded', 'true')
    expect(rail).toHaveAttribute('data-mobile-open', 'false')
    expect(sidebar).toHaveAttribute('data-mobile-open', 'true')
    expect(rail).toHaveAttribute('inert')
    expect(sidebar).not.toHaveAttribute('inert')
    expect(main).toHaveAttribute('inert')
    await waitFor(() =>
      expect(
        screen.getByRole('link', { name: 'First sidebar destination' }),
      ).toHaveFocus(),
    )

    fireEvent.keyDown(document, { key: 'Tab' })
    expect(
      screen.getByRole('button', { name: 'Last sidebar action' }),
    ).toHaveFocus()
    fireEvent.keyDown(document, { key: 'Tab' })
    expect(closeButton).toHaveFocus()

    fireEvent.keyDown(document, { key: 'Escape' })
    expect(sidebarButton).toHaveAttribute('aria-expanded', 'false')
    expect(
      screen.queryByRole('button', { name: 'Close navigation' }),
    ).not.toBeInTheDocument()
    expect(sidebarButton).toHaveFocus()
    expect(rail).not.toHaveAttribute('inert')
    expect(sidebar).not.toHaveAttribute('inert')
    expect(main).not.toHaveAttribute('inert')
    expect(context).not.toHaveAttribute('inert')
    expect(skipLink).not.toHaveAttribute('inert')
  })

  it('closes the drawer from the backdrop and restores its trigger focus', () => {
    render(
      <AppShell project={project} user={user}>
        <h1>Workspace</h1>
      </AppShell>,
    )

    const projectsButton = screen.getByRole('button', { name: 'Projects' })

    fireEvent.click(projectsButton)
    fireEvent.click(screen.getByRole('button', { name: 'Close navigation' }))
    expect(projectsButton).toHaveAttribute('aria-expanded', 'false')
    expect(projectsButton).toHaveFocus()
  })

  it('opens context as a labelled modal and restores the Details trigger', async () => {
    render(
      <AppShell
        context={
          <aside aria-label="Project context" className="context-drawer">
            <button type="button">Context action</button>
          </aside>
        }
        project={project}
        user={user}
      >
        <button type="button">Main action</button>
      </AppShell>,
    )

    const detailsButton = screen.getByRole('button', { name: 'Details' })
    fireEvent.click(detailsButton)

    expect(screen.getByRole('dialog', { name: 'Details' })).toHaveAttribute(
      'aria-modal',
      'true',
    )
    expect(document.querySelector('#main-workspace')).toHaveAttribute('inert')
    await waitFor(() =>
      expect(screen.getByRole('button', { name: 'Close Details' })).toHaveFocus(),
    )

    fireEvent.keyDown(document, { key: 'Escape' })
    expect(detailsButton).toHaveAttribute('aria-expanded', 'false')
    expect(detailsButton).toHaveFocus()
    expect(document.querySelector('#main-workspace')).not.toHaveAttribute('inert')
  })
})
