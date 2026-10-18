import { Suspense, useEffect, useId, useMemo, useRef, useState, type KeyboardEvent, type ReactNode } from 'react'
import { Link, Navigate, useLocation, useNavigate, useParams } from 'react-router-dom'

import { useAuthenticatedSession, type AuthenticatedSession } from '../auth/session'
import { ProjectPageError } from '../projects/ProjectPageStates'
import { useProject, useProjects } from '../projects/hooks'
import { projectLocationHref } from '../projects/locations'
import type { Project } from '../projects/types'
import { AppShell } from '../shell/AppShell'
import { Button, EmptyState, Icon, SettingsSkeleton } from '../ui'
import { useSettingsRegistry } from './api'
import { isArchiving } from './archiving'
import { accountSettingsHref, projectSettingsHref, workspaceSettingsHref } from './paths'
import { GitHubReturnBanner } from './GitHubReturnBanner'
import { parseSettingsPath } from './route'
import { GROUP_LABELS, SETTINGS_PAGES, findPage, pagesIn, type SettingsPage } from './registry'
import type { SettingsGroup, SettingsPageProps, SettingsVisibility } from './types'
import { defaultWorkspace, useSwitchWorkspace } from './workspaceChoice'
import './settings.css'

const LAST_PROJECT_KEY = 'agent-workspace:settings:project'

function rememberedProject() {
  try {
    return window.sessionStorage.getItem(LAST_PROJECT_KEY)
  } catch {
    return null
  }
}

function rememberProject(projectId: string) {
  try {
    window.sessionStorage.setItem(LAST_PROJECT_KEY, projectId)
  } catch {
    // Only a convenience: settings open on the first Project without it.
  }
}

function hrefFor(entry: SettingsPage, projectId: string | null) {
  if (entry.group === 'account') return accountSettingsHref(entry.id)
  if (entry.group === 'workspace') return workspaceSettingsHref(entry.id)
  return projectId ? projectSettingsHref(projectId, entry.id) : null
}

function organizationFor(session: AuthenticatedSession, project: Project | null) {
  return project ? (session.organizations.find((organization) => organization.id === project.organizationId) ?? null) : null
}

/** /settings/*: one area for your account, the workspace and each Project. */
export default function SettingsArea() {
  const session = useAuthenticatedSession()
  const { '*': splat = '' } = useParams()
  const location = useLocation()
  const navigate = useNavigate()
  const route = parseSettingsPath(splat, location.search)
  const projectList = useProjects()
  const projects = useMemo(() => projectList.data?.projects ?? [], [projectList.data])
  const workspace = defaultWorkspace(session)
  const [navOpen, setNavOpen] = useState(false)

  const routeProjectId = route.kind === 'page' && route.group === 'project' ? route.projectId : null
  const cameFrom = (location.state as { fromProjectId?: unknown } | null)?.fromProjectId
  const fromProjectId = typeof cameFrom === 'string' && projects.some((project) => project.id === cameFrom) ? cameFrom : null
  const preferredProjectId =
    routeProjectId ??
    fromProjectId ??
    projects.find((project) => project.id === rememberedProject())?.id ??
    projects[0]?.id ??
    null
  const remembered = routeProjectId ?? fromProjectId
  useEffect(() => {
    if (remembered) rememberProject(remembered)
  }, [remembered])

  const routeProject = useProject(routeProjectId ?? '')
  const pickerProject =
    (routeProject.data?.id === preferredProjectId ? routeProject.data : null) ??
    projects.find((project) => project.id === preferredProjectId) ??
    null

  const contexts: Record<SettingsGroup, SettingsVisibility> = {
    account: { session, workspace, project: null, projects },
    workspace: { session, workspace, project: null, projects },
    project: { session, workspace: organizationFor(session, pickerProject), project: pickerProject, projects },
  }

  // Close the phone menu whenever the page changes.
  const [shownPath, setShownPath] = useState(location.pathname)
  if (shownPath !== location.pathname) {
    setShownPath(location.pathname)
    setNavOpen(false)
  }

  if (route.kind === 'redirect') {
    return <Navigate replace state={location.state} to={`${route.to}${location.search}`} />
  }
  if (route.group === 'project' && !route.projectId) {
    if (projectList.isPending) return <SettingsFrame session={session}><SettingsSkeleton label="Loading Projects" /></SettingsFrame>
    if (preferredProjectId) return <Navigate replace to={projectSettingsHref(preferredProjectId)} />
  }

  const group = route.group
  const entry = findPage(group, route.section)
  let content
  if (group === 'project' && !route.projectId) {
    content = (
      <EmptyState icon="settings" title="No Projects yet">
        Project settings appear here once you create or join a Project.
      </EmptyState>
    )
  } else if (group === 'workspace' && !workspace) {
    content = (
      <EmptyState icon="users" title="You’re not in a workspace">
        Join a workspace with an invitation link to see its settings.
      </EmptyState>
    )
  } else if (group === 'project') {
    content = (
      <ProjectScope
        entry={entry}
        projects={projects}
        query={routeProject}
        session={session}
        section={route.section}
      />
    )
  } else {
    const context = contexts[group]
    if (entry && projectList.isPending && !(entry.visible?.(context) ?? true)) {
      // Some pages depend on the person's Projects; wait for them before deciding.
      content = <SettingsSkeleton label="Loading settings" />
    } else if (!entry || !(entry.visible?.(context) ?? true)) {
      const first = pagesIn(group, context)[0]
      return <Navigate replace to={first ? (hrefFor(first, null) ?? accountSettingsHref('profile')) : accountSettingsHref('profile')} />
    } else {
      content = <PageBody entry={entry} props={{ session, workspace, project: null, projects }} />
    }
  }

  const current = entry ? `${GROUP_LABELS[group]} · ${entry.label}` : GROUP_LABELS[group]

  return (
    <SettingsFrame session={session}>
      <div className={navOpen ? 'aw-settings is-nav-open' : 'aw-settings'}>
        <button
          aria-controls="settings-nav"
          aria-expanded={navOpen}
          className="aw-settings__toggle"
          onClick={() => setNavOpen((open) => !open)}
          type="button"
        >
          <Icon name="settings" />
          <span className="aw-settings__toggle-label">{current}</span>
          <Icon name="chevron-down" />
        </button>
        <SettingsNav
          contexts={contexts}
          currentGroup={group}
          currentSection={route.section}
          onPickProject={(projectId) =>
            navigate(projectSettingsHref(projectId, group === 'project' ? route.section : 'general'))
          }
          pickerProject={pickerProject}
          projects={projects}
          session={session}
          workspace={workspace}
        />
        <div className="aw-settings__content">
          {!(group === 'project' && route.section === 'repositories') && (
            <GitHubReturnBanner className="aw-settings__notice" key={location.pathname} />
          )}
          {content}
        </div>
      </div>
    </SettingsFrame>
  )
}

function SettingsFrame({ children, session }: { children: ReactNode; session: AuthenticatedSession }) {
  return (
    <AppShell mainClassName="aw-main--page" user={session.user}>
      {children}
    </AppShell>
  )
}

function PageBody({ entry, props }: { entry: SettingsPage; props: SettingsPageProps }) {
  const Page = entry.component
  return (
    <div className="aw-page aw-page--settings aw-settings__page">
      <Suspense fallback={<SettingsSkeleton label={`Loading ${entry.label.toLocaleLowerCase()}`} />}>
        <Page {...props} />
      </Suspense>
    </div>
  )
}

/**
 * A Project page: loads the Project and checks you can use the page. It doesn't
 * remember settings as where you were, or the Project's tile would lead back here.
 */
function ProjectScope({
  entry,
  projects,
  query,
  section,
  session,
}: {
  entry: SettingsPage | null
  projects: Project[]
  query: ReturnType<typeof useProject>
  section: string
  session: AuthenticatedSession
}) {
  const project = query.data ?? null
  if (query.isPending) return <SettingsSkeleton label="Loading Project" />
  if (query.isError || !project) return <ProjectPageError onRetry={() => void query.refetch()} />
  if (project.archivedAt && !isArchiving(project.id)) {
    return <Navigate replace to={projectLocationHref(project.id, { kind: 'project_overview' })} />
  }
  const workspace = organizationFor(session, project)
  const context = { session, workspace, project, projects }
  if (!entry || !(entry.visible?.(context) ?? true)) {
    if (section === 'general') return null
    return <Navigate replace to={projectSettingsHref(project.id)} />
  }
  return (
    <PageBody entry={entry} props={{ session, workspace, project, projects }} />
  )
}

function SettingsNav({
  contexts,
  currentGroup,
  currentSection,
  onPickProject,
  pickerProject,
  projects,
  session,
  workspace,
}: {
  contexts: Record<SettingsGroup, SettingsVisibility>
  currentGroup: SettingsGroup
  currentSection: string
  onPickProject: (projectId: string) => void
  pickerProject: Project | null
  projects: Project[]
  session: AuthenticatedSession
  workspace: SettingsVisibility['workspace']
}) {
  const [query, setQuery] = useState('')
  const switchWorkspace = useSwitchWorkspace()
  const navigate = useNavigate()
  const workspaceId = useId()
  const projectId = useId()
  const searching = query.trim().length > 0

  const link = (entry: SettingsPage) => {
    const href = hrefFor(entry, pickerProject?.id ?? null)
    if (!href) return null
    const current = entry.group === currentGroup && entry.id === currentSection
    return (
      <li key={`${entry.group}:${entry.id}`}>
        <Link aria-current={current ? 'page' : undefined} className="aw-settings__link" to={href}>
          <Icon name={entry.icon} />
          <span>{entry.label}</span>
        </Link>
      </li>
    )
  }

  return (
    <nav aria-label="Settings" className="aw-settings__nav" id="settings-nav">
      <SettingsSearch contexts={contexts} onQuery={setQuery} pickerProjectId={pickerProject?.id ?? null} query={query} />
      {!searching && (
        <>
          <div className="aw-settings__group">
            <h2 className="aw-settings__grouplabel">{GROUP_LABELS.account}</h2>
            <ul aria-label="Account settings" className="aw-settings__links">
              {pagesIn('account', contexts.account).map(link)}
            </ul>
          </div>
          {workspace && (
            <div className="aw-settings__group">
              <h2 className="aw-settings__grouplabel">{GROUP_LABELS.workspace}</h2>
              {session.organizations.length > 1 && (
                <div className="aw-settings__picker">
                  <label className="visually-hidden" htmlFor={workspaceId}>
                    Workspace
                  </label>
                  <select
                    disabled={switchWorkspace.isPending}
                    id={workspaceId}
                    onChange={(event) =>
                      switchWorkspace.mutate(event.target.value, {
                        onSuccess: () => {
                          if (currentGroup === 'workspace') navigate(workspaceSettingsHref('general'))
                        },
                      })
                    }
                    value={workspace.id}
                  >
                    {session.organizations.map((organization) => (
                      <option key={organization.id} value={organization.id}>
                        {organization.name}
                      </option>
                    ))}
                  </select>
                  <Icon name="chevron-down" size={14} />
                </div>
              )}
              <ul aria-label="Workspace settings" className="aw-settings__links">
                {pagesIn('workspace', contexts.workspace).map(link)}
              </ul>
            </div>
          )}
          {projects.length > 0 && (
            <div className="aw-settings__group">
              <h2 className="aw-settings__grouplabel">{GROUP_LABELS.project}</h2>
              <div className="aw-settings__picker">
                <label className="visually-hidden" htmlFor={projectId}>
                  Project
                </label>
                <select id={projectId} onChange={(event) => onPickProject(event.target.value)} value={pickerProject?.id ?? ''}>
                  {!pickerProject && <option value="">Choose a Project</option>}
                  {pickerProject && !projects.some((project) => project.id === pickerProject.id) && (
                    <option value={pickerProject.id}>{pickerProject.name}</option>
                  )}
                  {projects.map((project) => (
                    <option key={project.id} value={project.id}>
                      {project.name}
                    </option>
                  ))}
                </select>
                <Icon name="chevron-down" size={14} />
              </div>
              {pickerProject && (
                <ul aria-label="Project settings" className="aw-settings__links">
                  {pagesIn('project', contexts.project).map(link)}
                </ul>
              )}
            </div>
          )}
        </>
      )}
    </nav>
  )
}

type SearchResult = { id: string; href: string; title: string; detail: string; icon: SettingsPage['icon'] }

function matches(text: string, terms: string[]) {
  const haystack = text.toLocaleLowerCase()
  return terms.every((term) => haystack.includes(term))
}

/** Finds pages by name and keywords, and individual settings by their label and description. */
function SettingsSearch({
  contexts,
  onQuery,
  pickerProjectId,
  query,
}: {
  contexts: Record<SettingsGroup, SettingsVisibility>
  onQuery: (query: string) => void
  pickerProjectId: string | null
  query: string
}) {
  const navigate = useNavigate()
  const listRef = useRef<HTMLUListElement>(null)
  const terms = query.trim().toLocaleLowerCase().split(/\s+/).filter(Boolean)
  const registry = useSettingsRegistry(terms.length > 0)
  const resultsId = useId()

  const results = useMemo<SearchResult[]>(() => {
    if (!terms.length) return []
    const visible = SETTINGS_PAGES.filter(
      (entry) => (entry.visible?.(contexts[entry.group]) ?? true) && hrefFor(entry, pickerProjectId),
    )
    const found: SearchResult[] = visible
      .filter((entry) => matches([entry.label, GROUP_LABELS[entry.group], ...entry.keywords].join(' '), terms))
      .map((entry) => ({
        id: `page:${entry.group}:${entry.id}`,
        href: hrefFor(entry, pickerProjectId) ?? '',
        title: entry.label,
        detail: GROUP_LABELS[entry.group],
        icon: entry.icon,
      }))
    for (const definition of registry.data ?? []) {
      if (!matches(`${definition.label} ${definition.description} ${definition.key}`, terms)) continue
      for (const entry of visible) {
        const scope = entry.group === 'project' ? 'project' : entry.group === 'workspace' ? 'workspace' : 'user'
        if (!entry.settings?.some((prefix) => definition.key.startsWith(prefix))) continue
        if (!definition.scopes.includes(scope)) continue
        found.push({
          id: `setting:${entry.group}:${definition.key}`,
          href: `${hrefFor(entry, pickerProjectId)}#setting-${definition.key}`,
          title: definition.label,
          detail: `${GROUP_LABELS[entry.group]} · ${entry.label}`,
          icon: entry.icon,
        })
      }
    }
    return found
    // `terms` is derived from `query`.
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [contexts, pickerProjectId, query, registry.data])

  const onKeyDown = (event: KeyboardEvent<HTMLInputElement>) => {
    if (event.key === 'Escape' && query) {
      event.preventDefault()
      onQuery('')
    } else if (event.key === 'Enter' && results[0]) {
      event.preventDefault()
      navigate(results[0].href)
      onQuery('')
    } else if (event.key === 'ArrowDown' && results.length) {
      event.preventDefault()
      listRef.current?.querySelector<HTMLElement>('a')?.focus()
    }
  }

  const onListKeyDown = (event: KeyboardEvent<HTMLUListElement>) => {
    const links = [...(listRef.current?.querySelectorAll<HTMLElement>('a') ?? [])]
    const index = links.indexOf(document.activeElement as HTMLElement)
    if (event.key === 'ArrowDown' || event.key === 'ArrowUp') {
      event.preventDefault()
      const next = index + (event.key === 'ArrowDown' ? 1 : -1)
      if (next < 0) document.getElementById(`${resultsId}-input`)?.focus()
      else links[Math.min(next, links.length - 1)]?.focus()
    } else if (event.key === 'Escape') {
      onQuery('')
      document.getElementById(`${resultsId}-input`)?.focus()
    }
  }

  return (
    <div className="aw-settings__search">
      <div className="aw-field__control">
        <span className="aw-field__prefix">
          <Icon name="search" size={14} />
        </span>
        <input
          aria-controls={terms.length ? resultsId : undefined}
          aria-label="Search settings"
          autoComplete="off"
          id={`${resultsId}-input`}
          onChange={(event) => onQuery(event.target.value)}
          onKeyDown={onKeyDown}
          placeholder="Search settings"
          type="search"
          value={query}
        />
      </div>
      {terms.length > 0 && (
        <div className="aw-settings__results">
          {results.length ? (
            <ul aria-label="Matching settings" id={resultsId} onKeyDown={onListKeyDown} ref={listRef}>
              {results.map((result) => (
                <li key={result.id}>
                  <Link className="aw-settings__link aw-settings__result" onClick={() => onQuery('')} to={result.href}>
                    <Icon name={result.icon} />
                    <span>
                      <span className="aw-settings__result-title">{result.title}</span>
                      <span className="aw-settings__result-detail">{result.detail}</span>
                    </span>
                  </Link>
                </li>
              ))}
            </ul>
          ) : registry.isPending ? (
            <p className="aw-settings__noresults" role="status">
              Searching…
            </p>
          ) : (
            <p className="aw-settings__noresults" role="status">
              Nothing matches “{query.trim()}”.
            </p>
          )}
          <Button onClick={() => onQuery('')} size="sm" variant="ghost">
            Clear search
          </Button>
        </div>
      )}
    </div>
  )
}
