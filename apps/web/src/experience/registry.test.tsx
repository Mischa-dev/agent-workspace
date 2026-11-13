/// <reference types="node" />
import { readFileSync, readdirSync, statSync } from 'node:fs'
import { join, relative } from 'node:path'

import { QueryClient, QueryClientProvider } from '@tanstack/react-query'
import { cleanup, render, screen } from '@testing-library/react'
import type { ReactNode } from 'react'
import { MemoryRouter } from 'react-router-dom'
import { afterEach, describe, expect, it, vi } from 'vitest'

import { queryKeys } from '../api/queryKeys'
import type { Project } from '../projects/types'
import { HomeSidebar } from '../shell/HomeSidebar'
import { ProjectSidebar } from '../shell/ProjectSidebar'
import { MemoryTabs, WorkTabs } from '../shell/ProjectTabs'
import { authenticatedSession, organization } from '../test/appHarness'
import { OWN_WORDS, layoutKeys, type EffectiveLayout } from './layout'
import { NOUN_IDS, PART_IDS } from './parts'
import { REGISTRY } from './registry'

// Keeps layouts (AWD-66) whole as the app grows: the ids the code names
// match registry.json, presets name only real parts, and every row of a
// project's sidebar and every tab goes away when the layout hides its part,
// so a new one can't be added without registering it.

const projectId = '019fb600-0000-7000-8000-000000000001'
const channelId = '019fb600-0000-7000-8000-000000000002'

const project: Project = {
  id: projectId,
  organizationId: organization.id,
  key: 'AW',
  name: 'Agent Workspace',
  archivedAt: null,
  role: 'owner',
  version: 1,
  attentionCount: 0,
  capabilities: { view: true, manage: true, archive: true, navigationWrite: true },
  navigation: {
    location: { kind: 'project_overview' },
    href: `/projects/${projectId}`,
    pinned: false,
    lastOpenedAt: '2026-10-28T08:00:00Z',
    version: 1,
  },
}

const sidebarData = {
  projectId,
  channels: [
    {
      id: channelId,
      name: 'General',
      archivedAt: null,
      highestSequence: 1,
      highestSeenSequence: 1,
      unread: false,
      threads: [
        {
          id: '019fb600-0000-7000-8000-000000000010',
          parentChannelId: channelId,
          originMessageId: null,
          name: 'Plans for the week',
          archivedAt: null,
          parentArchivedAt: null,
          highestSequence: 1,
          highestSeenSequence: 1,
          attention: { kind: 'settled', label: 'Seen' },
        },
      ],
      hasMoreThreads: true,
    },
  ],
  capabilities: { createChannel: true },
}

function layout(hidden: readonly string[]): { effective: EffectiveLayout } {
  return { effective: { source: 'project', preset: null, hidden: [...hidden], layoutHidden: [...hidden], words: {}, lead: [], showingHidden: false, modulesOff: [] } }
}

function renderWith(hidden: readonly string[], children: ReactNode) {
  vi.stubGlobal(
    'fetch',
    vi.fn((input: RequestInfo | URL) =>
      Promise.resolve(
        new Response(JSON.stringify(String(input).includes('/conversation-sidebar') ? sidebarData : {}), {
          headers: { 'content-type': 'application/json' },
        }),
      ),
    ),
  )
  const queryClient = new QueryClient({ defaultOptions: { queries: { retry: false } } })
  queryClient.setQueryData(queryKeys.session, authenticatedSession)
  queryClient.setQueryData(layoutKeys.project(projectId), layout(hidden))
  queryClient.setQueryData(layoutKeys.workspace(organization.id), layout(hidden))
  return render(
    <QueryClientProvider client={queryClient}>
      <MemoryRouter initialEntries={[`/projects/${projectId}`]}>{children}</MemoryRouter>
    </QueryClientProvider>,
  )
}

const linkNames = () => screen.queryAllByRole('link').map((link) => link.textContent?.trim() ?? '')

afterEach(() => {
  cleanup()
  vi.unstubAllGlobals()
})

describe('layout registry', () => {
  it('names the same parts and nouns as the code', () => {
    expect(REGISTRY.parts.map((part) => part.id)).toEqual([...PART_IDS])
    expect(REGISTRY.nouns.map((noun) => noun.id)).toEqual([...NOUN_IDS])
    for (const noun of REGISTRY.nouns) expect(OWN_WORDS[noun.id]).toEqual({ one: noun.one, many: noun.many })
    const groups = new Set(REGISTRY.groups.map((group) => group.id))
    for (const part of REGISTRY.parts) {
      expect(groups.has(part.group), `${part.id} is in group ${part.group}, which isn't listed`).toBe(true)
      expect(part.label && part.description, `${part.id} needs a label and a one-line description`).toBeTruthy()
    }
  })

  it('has at least 15 presets, each naming only parts and nouns that exist', () => {
    expect(REGISTRY.presets.length).toBeGreaterThanOrEqual(15)
    expect(new Set(REGISTRY.presets.map((preset) => preset.id)).size).toBe(REGISTRY.presets.length)
    const parts = new Set<string>(PART_IDS)
    const nouns = new Set<string>(NOUN_IDS)
    for (const preset of REGISTRY.presets) {
      expect(Boolean(preset.only) !== Boolean(preset.hide), `${preset.id} lists parts to show (only) or to hide (hide), not both`).toBe(true)
      for (const part of [...(preset.only ?? []), ...(preset.hide ?? []), ...(preset.lead ?? [])]) {
        expect(parts.has(part), `${preset.id} names ${part}, which isn't a part`).toBe(true)
      }
      for (const noun of Object.keys(preset.words ?? {})) expect(nouns.has(noun), `${preset.id} renames ${noun}`).toBe(true)
      expect(preset.name && preset.description && preset.tone, `${preset.id} needs a name, a description and a tone`).toBeTruthy()
    }
  })

  it('hides every row of a project’s sidebar whose part is hidden', async () => {
    const view = renderWith([], <ProjectSidebar mobileOpen={false} onNavigate={vi.fn()} project={project} />)
    await screen.findByText('Plans for the week')
    expect(linkNames()).toEqual(expect.arrayContaining(['Overview', 'Tasks', 'Pages', 'Runs', 'General', 'Plans for the week, Seen', 'All threads']))
    view.unmount()

    renderWith(PART_IDS, <ProjectSidebar mobileOpen={false} onNavigate={vi.fn()} project={project} />)
    await new Promise((resolve) => window.setTimeout(resolve, 50))
    // Overview is the project itself, so it always shows. Anything else here
    // is a row added without a part: register it in registry.json and gate
    // it with useProjectExperience(...).shows(part).
    expect(linkNames()).toEqual(['Overview'])
  })

  it('hides every tab whose part is hidden, and the tab bar once one tab is left', () => {
    const view = renderWith([], <WorkTabs current="items" projectId={projectId} />)
    expect(linkNames()).toEqual(['Tasks', 'Milestones', 'Timeline', 'Planning'])
    view.unmount()
    const pages = renderWith([], <MemoryTabs current="pages" projectId={projectId} />)
    expect(linkNames()).toEqual(['Pages', 'Decisions'])
    pages.unmount()

    renderWith(PART_IDS, <WorkTabs current="items" projectId={projectId} />)
    expect(linkNames()).toEqual([])
  })

  it('hides the Home sidebar’s rows that the workspace layout hides', () => {
    const view = renderWith([], <HomeSidebar mobileOpen={false} onNavigate={vi.fn()} />)
    expect(linkNames()).toEqual(['Home', 'Inbox', 'My work', 'Later', 'Runs', 'Pages', 'Whiteboards'])
    view.unmount()

    renderWith(PART_IDS, <HomeSidebar mobileOpen={false} onNavigate={vi.fn()} />)
    // Home and Inbox are where everything else starts, so they always show.
    expect(linkNames()).toEqual(['Home', 'Inbox'])
  })

  it('knows every sidebar and tab bar in the app', () => {
    const src = join(process.cwd(), 'src')
    const files = (dir: string): string[] =>
      readdirSync(dir).flatMap((name) => {
        const path = join(dir, name)
        if (statSync(path).isDirectory()) return files(path)
        return name.endsWith('.tsx') && !name.includes('.test.') ? [path] : []
      })
    const users = files(src)
      .filter((path) => /<(?:SidebarLink|PageTabs)\b/.test(readFileSync(path, 'utf8')))
      .map((path) => relative(src, path).split('\\').join('/'))
      .sort()
    // Checked above, or outside layouts: Messages (direct conversations)
    // belongs to no project or workspace layout, and a collection's tabs are
    // its own saved views, made by people, not parts of the app. A new
    // sidebar or tab bar takes part in layouts: gate its entries on parts,
    // test it here, and add it to this list.
    expect(users).toEqual([
      'collections/CollectionPage.tsx',
      'direct/DirectSidebar.tsx',
      'shell/HomeSidebar.tsx',
      'shell/ProjectSidebar.tsx',
      'shell/ProjectTabs.tsx',
    ])
  })
})
