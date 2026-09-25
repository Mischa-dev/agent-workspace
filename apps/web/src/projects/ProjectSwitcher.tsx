import { useEffect, useMemo, useRef, useState, type CSSProperties, type KeyboardEvent, type MouseEvent } from 'react'
import { createPortal } from 'react-dom'

import { Badge, Icon, swatchFor } from '../ui'
import { useProjectSearch } from './hooks'
import { projectLocationHref } from './locations'
import type { Project } from './types'

/** Find a Project by name or key. Opens from the rail's search tile or Cmd/Ctrl+K. */
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
  onSelect: (project: Project, event: MouseEvent<HTMLAnchorElement>) => void
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
  const candidateProjects = settledQuery.trim() && search.data ? search.data.projects : projects
  const filteredProjects = useMemo(() => {
    const normalized = query.trim().toLocaleLowerCase()
    if (!normalized) return candidateProjects
    return candidateProjects.filter(
      (project) =>
        project.name.toLocaleLowerCase().includes(normalized) || project.key.toLocaleLowerCase().includes(normalized),
    )
  }, [candidateProjects, query])

  useEffect(() => {
    searchRef.current?.focus()
    const applicationRoot = document.querySelector<HTMLElement>('#root')
    const applicationWasInert = applicationRoot?.hasAttribute('inert') ?? false
    applicationRoot?.setAttribute('inert', '')
    return () => {
      if (!applicationWasInert) applicationRoot?.removeAttribute('inert')
    }
  }, [])

  const handleKeyDown = (event: KeyboardEvent<HTMLDivElement>) => {
    if (event.key === 'Escape') {
      event.preventDefault()
      event.stopPropagation()
      onClose()
      return
    }
    if (event.key === 'ArrowDown' || event.key === 'ArrowUp') {
      const links = Array.from(panelRef.current?.querySelectorAll<HTMLElement>('.aw-switcher__link') ?? [])
      if (!links.length) return
      event.preventDefault()
      const index = links.indexOf(document.activeElement as HTMLElement)
      const next = event.key === 'ArrowDown' ? index + 1 : index - 1
      links[(next + links.length) % links.length]?.focus()
      return
    }
    if (event.key !== 'Tab') return
    const focusable = Array.from(
      panelRef.current?.querySelectorAll<HTMLElement>('a[href], button:not([disabled]), input:not([disabled])') ?? [],
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

  return createPortal(
    <div
      className="aw-dialog-layer aw-dialog-layer--top"
      onMouseDown={(event) => {
        if (event.currentTarget === event.target) onClose()
      }}
    >
      <div aria-hidden="true" className="aw-scrim" />
      <div
        aria-labelledby="project-switcher-heading"
        aria-modal="true"
        className="aw-dialog aw-switcher"
        onKeyDown={handleKeyDown}
        ref={panelRef}
        role="dialog"
      >
        <h2 className="visually-hidden" id="project-switcher-heading">
          Find a Project
        </h2>
        <label className="aw-switcher__search" htmlFor="project-search">
          <span className="visually-hidden">Search Projects</span>
          <Icon name="search" size={18} />
          <input
            autoComplete="off"
            id="project-search"
            maxLength={100}
            onChange={(event) => setQuery(event.target.value)}
            placeholder="Find a Project by name or key"
            ref={searchRef}
            type="search"
            value={query}
          />
          <button aria-label="Close Project search" className="aw-iconbtn aw-iconbtn--sm" onClick={onClose} type="button">
            <Icon name="x" />
          </button>
        </label>
        {error && (
          <p className="aw-switcher__error" role="alert">
            {error}
          </p>
        )}
        <div aria-busy={search.isFetching} aria-live="polite" className="aw-switcher__results">
          {filteredProjects.length ? (
            <ul>
              {filteredProjects.map((project) => (
                <li className="aw-switcher__row" key={project.id}>
                  <a
                    aria-current={project.id === currentProjectId ? 'page' : undefined}
                    className="aw-switcher__link"
                    href={projectLocationHref(project.id, project.navigation.location)}
                    onClick={(event) => onSelect(project, event)}
                  >
                    <ProjectInitials project={project} />
                    <span className="aw-switcher__name">{project.name}</span>
                    <span className="aw-switcher__key">{project.key}</span>
                    {project.attentionCount > 0 && (
                      <Badge count={project.attentionCount} label={`${project.attentionCount} items need attention`} />
                    )}
                  </a>
                  <button
                    aria-label={`${project.navigation.pinned ? 'Unpin' : 'Pin'} ${project.name}`}
                    aria-pressed={project.navigation.pinned}
                    className="aw-iconbtn aw-iconbtn--sm aw-switcher__pin"
                    disabled={!project.capabilities.navigationWrite}
                    onClick={() => onTogglePin(project)}
                    type="button"
                  >
                    <Icon name="star" />
                  </button>
                </li>
              ))}
            </ul>
          ) : (
            <div className="aw-switcher__empty">
              <strong>No matching Projects</strong>
              <p>Try a Project name or its short key.</p>
            </div>
          )}
        </div>
      </div>
    </div>,
    document.body,
  )
}

/** A Project's key on its color: the same tile the rail shows. */
export function ProjectInitials({ project, size = 28 }: { project: Project; size?: number }) {
  return (
    <span
      aria-hidden="true"
      className="aw-space-mark"
      style={{ '--tile-bg': `var(--swatch-${swatchFor(project.id)})`, '--mark-size': `${size}px` } as CSSProperties}
    >
      {project.key.slice(0, 2)}
    </span>
  )
}
