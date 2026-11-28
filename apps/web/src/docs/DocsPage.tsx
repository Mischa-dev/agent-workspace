import { useMemo, useState, type CSSProperties } from 'react'
import { useQuery } from '@tanstack/react-query'
import { Link, useNavigate, useParams } from 'react-router-dom'

import type { DocEntry } from '../api/generated/operations'
import { opQuery, ranOrThrow, useOp } from '../api/ops'
import { spaceHref } from '../app/href'
import { useProjectExperience } from '../experience/layout'
import { ProjectPageError, ProjectPageLoading } from '../projects/ProjectPageStates'
import { useProject } from '../projects/hooks'
import { projectAreaHref } from '../projects/locations'
import { useShellPage } from '../shell/shellStore'
import { Button, ConfirmDialog, EmptyState, ErrorState, Icon, IconButton, ListSkeleton, Menu, PageHeader, countLabel, describeError, formatRelative, type MenuEntry } from '../ui'
import { DocAccessDialog, MoveDocDialog, NewDocDialog, RenameFolderDialog } from './DocDialogs'
import { MADE_KINDS, docHref, docRows, kindIcon, kindLabel, readOpened, writeOpened, type DocRow, type MadeKind } from './docsModel'
import './docs.css'

type Open =
  | { dialog: 'new'; kind: MadeKind | 'folder'; folderId: string | null }
  | { dialog: 'rename' | 'move' | 'access' | 'remove'; entry: DocEntry }

/** The lists each kind keeps of its own: templates, archived ones and the rest live there. */
const BY_KIND: { module: string; label: string; path: (projectId: string) => string }[] = [
  { module: 'pages', label: 'Pages', path: (projectId) => spaceHref(projectId, '/pages') },
  { module: 'sheets', label: 'Sheets', path: (projectId) => projectAreaHref(projectId, 'sheets') },
  { module: 'collections', label: 'Collections', path: (projectId) => projectAreaHref(projectId, 'collections') },
  { module: 'dashboards', label: 'Dashboards', path: (projectId) => projectAreaHref(projectId, 'dashboards') },
]

/**
 * A project's Docs: its pages, sheets, whiteboards, collections and
 * dashboards in one tree, with folders. Anything in it can be moved into a
 * folder, and a folder or a single document can be kept to chosen people.
 */
export default function DocsPage() {
  const { projectId = '' } = useParams()
  // Keyed by project, so the folders left open are read again for each one.
  return <Docs key={projectId} projectId={projectId} />
}

function Docs({ projectId }: { projectId: string }) {
  const project = useProject(projectId)
  const experience = useProjectExperience(projectId)
  const list = useQuery({ ...opQuery('doc_list', { spaceId: projectId, deep: true }), enabled: Boolean(projectId) })
  const navigate = useNavigate()
  const [opened, setOpened] = useState(() => readOpened(projectId))
  const [open, setOpen] = useState<Open | null>(null)
  const [removeError, setRemoveError] = useState<string | null>(null)
  const remove = useOp('doc_folder_update', { invalidates: ['doc_list'] })

  useShellPage({ project: project.data })

  const entries = useMemo(() => list.data?.entries ?? [], [list.data])
  const rows = useMemo(() => docRows(entries, opened), [entries, opened])
  const byId = useMemo(() => new Map(entries.map((entry) => [entry.id, entry])), [entries])

  if (project.isPending) return <ProjectPageLoading label="Loading docs" />
  if (project.isError) return <ProjectPageError error={project.error} onRetry={() => void project.refetch()} />

  // A layout may call pages something else (notes, drafts); every other kind keeps its name.
  const label = (kind: DocEntry['kind']) => (kind === 'page' ? experience.label('page') : kindLabel(kind))
  const live = !project.data.archivedAt
  const canEdit = new Set(live ? (list.data?.canEdit ?? []) : [])
  const makes = MADE_KINDS.filter((kind) => canEdit.has(kind))
  const canFolder = canEdit.size > 0

  const toggle = (id: string, on?: boolean) => {
    const next = new Set(opened)
    if (on ?? !next.has(id)) next.add(id)
    else next.delete(id)
    setOpened(next)
    writeOpened(projectId, next)
  }
  const newItems = (folderId: string | null): MenuEntry[] => [
    ...makes.map((kind) => ({ label: label(kind), icon: kindIcon(kind), onSelect: () => setOpen({ dialog: 'new', kind, folderId }) })),
    ...(makes.length && canFolder ? [{ separator: true as const }] : []),
    ...(canFolder ? [{ label: 'Folder', icon: 'folder' as const, onSelect: () => setOpen({ dialog: 'new', kind: 'folder' as const, folderId }) }] : []),
  ]
  const topNew = newItems(null)
  const newButton = topNew.length ? (
    <Menu
      items={topNew}
      label="New"
      placement="bottom-end"
      trigger={
        <Button icon="plus" variant="primary">
          New
        </Button>
      }
    />
  ) : null
  const kinds = BY_KIND.filter((kind) => experience.moduleOn(kind.module))
  const actions = (
    <>
      {kinds.length ? (
        <Menu
          items={kinds.map((kind) => ({ label: kind.module === 'pages' ? experience.label('page', 'many') : kind.label, onSelect: () => void navigate(kind.path(projectId)) }))}
          label="Browse one kind"
          placement="bottom-end"
          trigger={
            <Button iconEnd="chevron-down" variant="ghost">
              By kind
            </Button>
          }
        />
      ) : null}
      {newButton}
    </>
  )

  const rowMenu = ({ entry, holds }: DocRow): MenuEntry[] => {
    if (!canEdit.has(entry.kind) && !(entry.kind === 'folder' && canFolder)) {
      return entry.restricted ? [{ label: 'Who can see it', icon: 'lock', onSelect: () => setOpen({ dialog: 'access', entry }) }] : []
    }
    const insidePage = entry.kind === 'page' && byId.get(entry.parentId ?? '')?.kind === 'page'
    const items: MenuEntry[] = []
    if (entry.kind === 'folder') {
      const inside = newItems(entry.id)
      if (inside.length) items.push({ heading: 'New in this folder' }, ...inside, { separator: true })
      items.push({ label: 'Rename', icon: 'pencil', onSelect: () => setOpen({ dialog: 'rename', entry }) })
    }
    if (!insidePage) items.push({ label: 'Move', icon: 'folder', onSelect: () => setOpen({ dialog: 'move', entry }) })
    items.push({ label: 'Who can see it', icon: 'lock', onSelect: () => setOpen({ dialog: 'access', entry }) })
    if (entry.kind === 'folder') {
      items.push(
        { separator: true },
        {
          label: 'Remove folder',
          icon: 'trash',
          danger: true,
          disabled: holds > 0,
          onSelect: () => {
            setRemoveError(null)
            setOpen({ dialog: 'remove', entry })
          },
        },
      )
    }
    return items
  }

  const meta = ({ entry, holds }: DocRow) => {
    const parts = [label(entry.kind)]
    if (entry.kind === 'folder') parts.push(holds ? countLabel(holds, 'thing') : 'Empty')
    else if (holds) parts.push(`${countLabel(holds, experience.word('page'))} inside`)
    if (entry.restrictedHere) parts.push('Only chosen people')
    else if (entry.restricted) parts.push('Limited by its folder')
    if (entry.kind !== 'folder') parts.push(`changed ${formatRelative(entry.updatedAt)}`)
    return parts.join(' · ')
  }

  return (
    <div className="aw-page aw-docs">
      <PageHeader actions={actions} icon="folder" title="Docs" />
      {list.isPending ? <ListSkeleton label="Loading docs" rows={4} /> : null}
      {list.isError ? <ErrorState error={list.error} onRetry={() => void list.refetch()} title="Docs didn’t load" /> : null}
      {list.isSuccess && rows.length === 0 ? (
        <EmptyState action={newButton} icon="folder" title="Nothing here yet">
          Docs holds this project’s {experience.word('page', 'many')}, sheets, whiteboards, collections and dashboards together. Sort them into folders, and keep a folder or a single document to the people you choose.
        </EmptyState>
      ) : null}
      {rows.length ? (
        <ul aria-label="Docs" className="aw-rows aw-docs__rows">
          {rows.map((row) => {
            const { entry, depth, holds } = row
            const href = docHref(projectId, entry)
            const menu = rowMenu(row)
            return (
              <li className="aw-row aw-docs__row" key={entry.id} style={{ '--doc-depth': depth } as CSSProperties}>
                <span className="aw-docs__twist">
                  {holds ? (
                    <IconButton
                      aria-expanded={row.open}
                      icon={row.open ? 'chevron-down' : 'chevron-right'}
                      label={row.open ? `Close ${entry.title}` : `Open ${entry.title}`}
                      onClick={() => toggle(entry.id)}
                      size="sm"
                      tooltip={false}
                    />
                  ) : null}
                </span>
                <span className="aw-row__lead aw-docs__kind">
                  <Icon name={kindIcon(entry.kind)} size={18} />
                </span>
                <div className="aw-row__main">
                  <div className="aw-row__title">
                    <span className="aw-row__titleline">
                      {href ? (
                        <Link to={href}>{entry.title}</Link>
                      ) : (
                        <Button aria-expanded={holds ? row.open : undefined} className="aw-docs__folder" onClick={() => toggle(entry.id)} size="sm" variant="ghost">
                          {entry.title}
                        </Button>
                      )}
                      {entry.restricted ? (
                        <span className="aw-docs__lock" title={entry.restrictedHere ? 'Only chosen people can see it' : 'Only the people its folder names can see it'}>
                          <Icon name="lock" size={14} />
                          <span className="aw-sr-only">Restricted</span>
                        </span>
                      ) : null}
                    </span>
                  </div>
                  <div className="aw-row__meta">{meta(row)}</div>
                </div>
                {menu.length ? (
                  <div className="aw-row__actions">
                    <Menu items={menu} label={`Actions for ${entry.title}`} placement="bottom-end" trigger={<IconButton icon="more" label={`Actions for ${entry.title}`} size="sm" tooltip={false} />} />
                  </div>
                ) : null}
              </li>
            )
          })}
        </ul>
      ) : null}
      {open?.dialog === 'new' ? (
        <NewDocDialog
          entries={entries}
          folderId={open.folderId}
          kind={open.kind}
          noun={label(open.kind).toLowerCase()}
          onClose={() => setOpen(null)}
          onMade={(made) => {
            if (made.folderId) toggle(made.folderId, true)
            const href = made.kind === 'folder' ? null : docHref(projectId, { id: made.id, kind: made.kind })
            if (href) void navigate(href)
          }}
          projectId={projectId}
        />
      ) : null}
      {open?.dialog === 'rename' ? <RenameFolderDialog folder={open.entry} onClose={() => setOpen(null)} /> : null}
      {open?.dialog === 'move' ? <MoveDocDialog entries={entries} entry={open.entry} onClose={() => setOpen(null)} /> : null}
      {open?.dialog === 'access' ? <DocAccessDialog entry={open.entry} noun={label(open.entry.kind).toLowerCase()} onClose={() => setOpen(null)} projectId={projectId} /> : null}
      {open?.dialog === 'remove' ? (
        <ConfirmDialog
          busy={remove.isPending}
          confirmLabel="Remove folder"
          error={removeError}
          onClose={() => setOpen(null)}
          onConfirm={() => {
            setRemoveError(null)
            remove.mutate(
              { folderId: open.entry.id, remove: true },
              {
                onSuccess: (value) => {
                  try {
                    ranOrThrow(value, 'Removing it')
                    setOpen(null)
                  } catch (error) {
                    setRemoveError(describeError(error).message)
                  }
                },
                onError: (error) => setRemoveError(describeError(error).message),
              },
            )
          }}
          open
          title={`Remove “${open.entry.title}”?`}
        >
          The folder is empty, so nothing else is removed.
        </ConfirmDialog>
      ) : null}
    </div>
  )
}
