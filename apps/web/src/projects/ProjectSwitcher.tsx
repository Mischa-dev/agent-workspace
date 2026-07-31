import { useEffect, useMemo, useRef, useState } from 'react'

import { useProjectSearch } from './hooks'
import { projectLocationHref } from './locations'
import type { Project } from './types'

export function ProjectSwitcher({
  currentProjectId,
  projects,
  error,
  onClose,
  onSelect,
  onTogglePin,
}: {
  currentProjectId: string | null
  projects: Project[]
  error?: string | null
  onClose: () => void
  onSelect: (project: Project, event: React.MouseEvent<HTMLAnchorElement>) => void
  onTogglePin: (project: Project) => void
}) {
  const panelRef = useRef<HTMLDivElement>(null)
  const searchRef = useRef<HTMLInputElement>(null)
  const [query, setQuery] = useState('')
  const [settledQuery, setSettledQuery] = useState('')
  const search = useProjectSearch(settledQuery)
  useEffect(() => {
    const timer = window.setTimeout(() => setSettledQuery(query), 200)
    return () => window.clearTimeout(timer)
  }, [query])
  const candidateProjects =
    settledQuery.trim() && search.data ? search.data.projects : projects
  const filteredProjects = useMemo(() => {
    const normalized = query.trim().toLocaleLowerCase()
    if (!normalized) return candidateProjects
    return candidateProjects.filter(
      (project) =>
        project.name.toLocaleLowerCase().includes(normalized) ||
        project.key.toLocaleLowerCase().includes(normalized),
    )
  }, [candidateProjects, query])

  useEffect(() => {
    searchRef.current?.focus()
  }, [])

  const handleKeyDown = (event: React.KeyboardEvent<HTMLDivElement>) => {
    if (event.key === 'Escape') {
      event.preventDefault()
      event.stopPropagation()
      onClose()
      return
    }
    if (event.key !== 'Tab') return
    const focusable = Array.from(
      panelRef.current?.querySelectorAll<HTMLElement>(
        'a[href], button:not([disabled]), input:not([disabled])',
      ) ?? [],
    )
    if (!focusable.length) return
    const first = focusable[0]
    const last = focusable[focusable.length - 1]
    if (event.shiftKey && document.activeElement === first) {
      event.preventDefault()
      last.focus()
    } else if (!event.shiftKey && document.activeElement === last) {
      event.preventDefault()
      first.focus()
    }
  }

  return (
    <div
      className="switcher-backdrop"
      onMouseDown={(event) => {
        if (event.currentTarget === event.target) onClose()
      }}
    >
      <div
        aria-labelledby="project-switcher-heading"
        aria-modal="true"
        className="project-switcher"
        onKeyDown={handleKeyDown}
        ref={panelRef}
        role="dialog"
      >
        <header>
          <div>
            <span className="section-label">Navigate</span>
            <h2 id="project-switcher-heading">Find a Project</h2>
          </div>
          <button
            aria-label="Close Project search"
            className="icon-button"
            onClick={onClose}
            type="button"
          >
            ×
          </button>
        </header>
        <label className="search-field" htmlFor="project-search">
          <span className="visually-hidden">Search Projects</span>
          <span aria-hidden="true">⌕</span>
          <input
            autoComplete="off"
            id="project-search"
            maxLength={100}
            onChange={(event) => setQuery(event.target.value)}
            placeholder="Search by name or key"
            ref={searchRef}
            type="search"
            value={query}
          />
        </label>
        {error && (
          <p className="inline-error switcher-error" role="alert">
            {error}
          </p>
        )}
        <div
          aria-busy={search.isFetching}
          aria-live="polite"
          className="switcher-results"
          data-empty={!filteredProjects.length}
        >
          {filteredProjects.length ? (
            <ul>
              {filteredProjects.map((project) => (
                <li key={project.id}>
                  <a
                    aria-current={
                      project.id === currentProjectId ? 'page' : undefined
                    }
                    href={projectLocationHref(
                      project.id,
                      project.navigation.location,
                    )}
                    onClick={(event) => onSelect(project, event)}
                  >
                    <ProjectInitials project={project} />
                    <span>
                      <strong>{project.name}</strong>
                      <small>{project.key}</small>
                    </span>
                    {project.attentionCount > 0 && (
                      <span
                        aria-label={`${project.attentionCount} items need attention`}
                        className="attention-count"
                      >
                        {project.attentionCount > 99
                          ? '99+'
                          : project.attentionCount}
                      </span>
                    )}
                  </a>
                  <button
                    aria-label={`${project.navigation.pinned ? 'Unpin' : 'Pin'} ${project.name}`}
                    className="pin-button"
                    disabled={!project.capabilities.navigationWrite}
                    onClick={() => onTogglePin(project)}
                    type="button"
                  >
                    <span aria-hidden="true">
                      {project.navigation.pinned ? '★' : '☆'}
                    </span>
                  </button>
                </li>
              ))}
            </ul>
          ) : (
            <div className="switcher-empty">
              <strong>No matching Projects</strong>
              <p>Try a Project name or its short key.</p>
            </div>
          )}
        </div>
      </div>
    </div>
  )
}

export function ProjectInitials({ project }: { project: Project }) {
  return (
    <span aria-hidden="true" className="project-initials">
      {project.key.slice(0, 2)}
    </span>
  )
}
