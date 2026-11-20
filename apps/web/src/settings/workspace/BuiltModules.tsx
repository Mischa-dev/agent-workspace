import { useQuery, useQueryClient } from '@tanstack/react-query'
import { useState, type FormEvent, type ReactNode } from 'react'

import { callOp, invalidateOps, opKey, useOp, type OpInput, type OpOutput } from '../../api/ops'
import type { OpName } from '../../api/generated/operations'
import { refreshLayouts } from '../../experience/api'
import { useCollections } from '../../collections/api'
import type { Project } from '../../projects/types'
import {
  Banner,
  Button,
  Checkbox,
  ConfirmDialog,
  Dialog,
  EmptyState,
  ErrorState,
  Icon,
  IconButton,
  ListSkeleton,
  Menu,
  Row,
  RowList,
  Section,
  SelectField,
  Switch,
  TextField,
  describeError,
  showToast,
  type IconName,
} from '../../ui'

import './modules.css'

// M1.7: Settings › Modules › Installed › Built here. Modules people build
// from parts they already have (collections and pages in a space), shown in
// the sidebar by the entries they choose, and switched on and off as one
// without losing anything. Forms and automations join a module through its
// agent tools (module_part_link); the builder covers what most modules are.

type Summary = OpOutput<'module_list'>['built'][number]

/** A read through `callOp`, keyed as every operation read is. */
function read<N extends OpName>(name: N, input: OpInput<N>, workspaceId: string) {
  return { queryKey: opKey(name, input, workspaceId), queryFn: (): Promise<OpOutput<N>> => callOp(name, input, { workspaceId }) }
}
type Opened = OpOutput<'module_open'>

/** The icons a module can wear: the shared set's ones that read as a place. */
const MODULE_ICONS: { value: IconName; label: string }[] = [
  { value: 'table', label: 'Table' },
  { value: 'list', label: 'List' },
  { value: 'board', label: 'Board' },
  { value: 'checklist', label: 'Checklist' },
  { value: 'chart', label: 'Chart' },
  { value: 'inbox', label: 'Inbox' },
  { value: 'bookmark', label: 'Bookmark' },
  { value: 'star', label: 'Star' },
  { value: 'activity', label: 'Activity' },
  { value: 'users', label: 'People' },
  { value: 'repo', label: 'Code' },
  { value: 'whiteboard', label: 'Whiteboard' },
]

function moduleIcon(icon: string | null | undefined): IconName {
  return MODULE_ICONS.find((choice) => choice.value === icon)?.value ?? 'table'
}

/** A key from a name: lowercase letters, digits and underscores, starting with a letter. */
function keyFrom(name: string) {
  const key = name
    .toLowerCase()
    .normalize('NFKD')
    .replace(/[^a-z0-9]+/g, '_')
    .replace(/^[^a-z]+/, '')
    .replace(/_+$/, '')
  return key.slice(0, 40)
}

export function BuiltModules({ canBuild, projects, workspaceId }: { canBuild: boolean; projects: Project[]; workspaceId: string }) {
  const queryClient = useQueryClient()
  const list = useQuery(read('module_list', {}, workspaceId))
  const [building, setBuilding] = useState<{ moduleId: string | null } | null>(null)
  const [archiving, setArchiving] = useState<Summary | null>(null)
  const settle = () => {
    void refreshLayouts(queryClient)
  }
  const toggle = useOp('module_set', {
    workspaceId,
    invalidates: ['module_list', 'module_open'],
    onSuccess: (module) => {
      settle()
      showToast({
        tone: 'success',
        title: module.on ? `${module.name} is on` : `${module.name} is off`,
        description: module.on ? undefined : 'Nothing in it was deleted.',
      })
    },
  })
  const archive = useOp('module_archive', {
    workspaceId,
    invalidates: ['module_list', 'module_open'],
    onSuccess: (module) => {
      setArchiving(null)
      settle()
      showToast({ tone: 'success', title: `${module.name} archived` })
    },
  })
  const spaceName = (id: string | null | undefined) => (id ? projects.find((project) => project.id === id)?.name : null)

  const add = canBuild ? (
    <Button icon="plus" onClick={() => setBuilding({ moduleId: null })} size="sm">
      Build a module
    </Button>
  ) : null
  const section = (body: ReactNode, actions?: ReactNode) => (
    <Section
      actions={actions}
      description="Your own modules, built from collections and pages you already have. Each has its place in the sidebar and switches on and off as one."
      title="Built here"
    >
      {body}
    </Section>
  )
  if (list.isPending) return section(<ListSkeleton label="Loading built modules" rows={2} />)
  if (list.isError) return section(<ErrorState error={list.error} onRetry={() => void list.refetch()} title="Built modules didn’t load" />)
  const built = list.data.built

  return (
    <>
      {built.length === 0
        ? section(
            <EmptyState action={add} icon="table" title="Nothing built here yet">
              Bundle collections and pages you already have into a module of your own, with its own place in the sidebar.
            </EmptyState>,
          )
        : section(
            <RowList>
              {built.map((module) => {
                const where = spaceName(module.spaceId)
                return (
                  <Row
                    actions={
                      <span className="aw-row__metaline">
                        <Switch
                          checked={module.on}
                          disabled={!canBuild || toggle.isPending}
                          label={<span className="visually-hidden">{module.name}</span>}
                          onChange={(on) => toggle.mutate({ moduleId: module.id, on })}
                        />
                        {canBuild ? (
                          <Menu
                            items={[
                              {
                                label: 'Edit…',
                                icon: 'pencil',
                                onSelect: () => window.requestAnimationFrame(() => setBuilding({ moduleId: module.id })),
                              },
                              { separator: true },
                              {
                                label: 'Archive…',
                                icon: 'archive',
                                danger: true,
                                onSelect: () => window.requestAnimationFrame(() => setArchiving(module)),
                              },
                            ]}
                            label={`${module.name} actions`}
                            trigger={<IconButton icon="more" label={`${module.name} actions`} size="sm" />}
                          />
                        ) : null}
                      </span>
                    }
                    key={module.id}
                    leading={<Icon name={moduleIcon(module.icon)} size={18} />}
                    meta={[module.description, `${module.parts} part${module.parts === 1 ? '' : 's'}`, where ? `In ${where}` : 'Whole workspace']
                      .filter(Boolean)
                      .join(' · ')}
                    title={module.name}
                  />
                )
              })}
            </RowList>,
            add,
          )}
      {building ? (
        <ModuleBuilder
          moduleId={building.moduleId}
          onClose={() => setBuilding(null)}
          onSaved={() => {
            setBuilding(null)
            settle()
            void invalidateOps(queryClient, ['module_list', 'module_open'], workspaceId)
          }}
          projects={projects.filter((project) => project.organizationId === workspaceId && !project.archivedAt)}
          workspaceId={workspaceId}
        />
      ) : null}
      <ConfirmDialog
        busy={archive.isPending}
        confirmLabel="Archive"
        error={archive.isError ? describeError(archive.error).message : undefined}
        onClose={() => {
          setArchiving(null)
          archive.reset()
        }}
        onConfirm={() => archiving && archive.mutate({ moduleId: archiving.id, parts: 'keep' })}
        open={archiving !== null}
        title={`Archive ${archiving?.name ?? 'the module'}?`}
        tone="danger"
      >
        It leaves the sidebar and its switch goes away. Its collections and pages stay where they are, and it can be restored.
      </ConfirmDialog>
    </>
  )
}

type PartChoice = { kind: 'collection' | 'page'; id: string; name: string }
type Picked = { inModule: boolean; inSidebar: boolean }

const partKey = (part: { kind: string; id: string }) => `${part.kind}:${part.id}`

function navTarget(part: PartChoice) {
  return part.kind === 'collection' ? { kind: 'collection', collection: part.id } : { kind: 'page', page: part.id }
}

function navPoints(target: unknown): string | null {
  const value = target as { kind?: string; collection?: string; page?: string } | null
  if (value?.kind === 'collection' && value.collection) return `collection:${value.collection}`
  if (value?.kind === 'page' && value.page) return `page:${value.page}`
  return null
}

/** Build a module, or change one: its name, what it holds, and which of those the sidebar shows. */
export function ModuleBuilder({
  moduleId,
  onClose,
  onSaved,
  projects,
  workspaceId,
}: {
  moduleId: string | null
  onClose: () => void
  onSaved: () => void
  projects: Project[]
  workspaceId: string
}) {
  const opened = useQuery({ ...read('module_open', { moduleId: moduleId ?? '' }, workspaceId), enabled: moduleId !== null })
  if (moduleId && opened.isPending) {
    return (
      <Dialog onClose={onClose} open title="Edit module">
        <ListSkeleton label="Loading the module" rows={3} />
      </Dialog>
    )
  }
  if (moduleId && opened.isError) {
    return (
      <Dialog onClose={onClose} open title="Edit module">
        <ErrorState error={opened.error} onRetry={() => void opened.refetch()} title="The module didn’t load" />
      </Dialog>
    )
  }
  return (
    <BuilderForm
      key={moduleId ?? 'new'}
      onClose={onClose}
      onSaved={onSaved}
      opened={moduleId ? (opened.data ?? null) : null}
      projects={projects}
      workspaceId={workspaceId}
    />
  )
}

function BuilderForm({
  onClose,
  onSaved,
  opened,
  projects,
  workspaceId,
}: {
  onClose: () => void
  onSaved: () => void
  opened: Opened | null
  projects: Project[]
  workspaceId: string
}) {
  const [name, setName] = useState(opened?.name ?? '')
  const [key, setKey] = useState<string | null>(null)
  const [description, setDescription] = useState(opened?.description ?? '')
  const [icon, setIcon] = useState<IconName>(moduleIcon(opened?.icon))
  const [spaceId, setSpaceId] = useState(opened?.spaceId ?? projects[0]?.id ?? '')
  const [picked, setPicked] = useState<Record<string, Picked>>(() => {
    const start: Record<string, Picked> = {}
    const shown = new Set((opened?.nav ?? []).map((entry) => navPoints(entry.target)).filter(Boolean))
    for (const part of opened?.parts ?? []) start[partKey(part)] = { inModule: true, inSidebar: shown.has(partKey(part)) }
    return start
  })
  const [saving, setSaving] = useState(false)
  const [error, setError] = useState<string | null>(null)
  const [dirty, setDirty] = useState(false)
  const collections = useCollections(spaceId)
  const pages = useQuery({ ...read('page_list', { projectId: spaceId }, workspaceId), enabled: Boolean(spaceId) })
  const choices: PartChoice[] = [
    ...(collections.data?.collections ?? []).map((collection) => ({ kind: 'collection' as const, id: collection.id, name: collection.name })),
    ...(pages.data?.pages ?? []).filter((page) => !page.isTemplate).map((page) => ({ kind: 'page' as const, id: page.id, name: page.title })),
  ]
  const editing = opened !== null
  const shownKey = key ?? keyFrom(name)
  const change = (part: PartChoice, next: Partial<Picked>) => {
    setDirty(true)
    setPicked((current) => {
      const was = current[partKey(part)] ?? { inModule: false, inSidebar: false }
      const now = { ...was, ...next }
      if (!now.inModule) now.inSidebar = false
      if (next.inModule && !was.inModule) now.inSidebar = true
      return { ...current, [partKey(part)]: now }
    })
  }

  const save = async (event: FormEvent<HTMLFormElement>) => {
    event.preventDefault()
    if (!name.trim()) {
      setError('Give it a name.')
      return
    }
    setSaving(true)
    setError(null)
    const chosen = choices.filter((part) => picked[partKey(part)]?.inModule)
    const sidebar = chosen.filter((part) => picked[partKey(part)]?.inSidebar)
    const nav = sidebar.map((part) => ({ label: part.name.slice(0, 40), icon: part.kind === 'page' ? null : icon, target: navTarget(part) }))
    try {
      if (!opened) {
        await callOp(
          'module_create',
          {
            key: shownKey,
            name: name.trim(),
            description: description.trim() || null,
            icon,
            spaceId: spaceId || null,
            parts: chosen.map((part) => ({ kind: part.kind, id: part.id })),
            nav,
          },
          { workspaceId },
        )
      } else {
        const had = new Set(opened.parts.map(partKey))
        const shownBefore = new Map(opened.nav.map((entry) => [navPoints(entry.target), entry.id]))
        await callOp(
          'module_update',
          {
            moduleId: opened.id,
            expectedRevision: opened.revision,
            name: name.trim(),
            description: description.trim(),
            icon,
            nav: nav.filter((entry) => !shownBefore.has(navPoints(entry.target))),
            // Only entries the builder offers can be taken off here; a
            // form's entry, or a part from another space, stays.
            removeNav: [...shownBefore.entries()]
              .filter(([points]) => points !== null && choices.some((part) => partKey(part) === points) && !sidebar.some((part) => partKey(part) === points))
              .map(([, id]) => id),
          },
          { workspaceId },
        )
        for (const part of chosen.filter((part) => !had.has(partKey(part)))) {
          await callOp('module_part_link', { moduleId: opened.id, part: { kind: part.kind, id: part.id } }, { workspaceId })
        }
        const kept = new Set(chosen.map(partKey))
        for (const part of opened.parts.filter((part) => !kept.has(partKey(part)) && choices.some((choice) => partKey(choice) === partKey(part)))) {
          await callOp('module_part_unlink', { moduleId: opened.id, part: { kind: part.kind, id: part.id } }, { workspaceId })
        }
      }
      showToast({
        tone: 'success',
        title: opened ? `${name.trim()} saved` : `${name.trim()} built`,
        description: sidebar.length ? 'Its entries are in the sidebar.' : undefined,
      })
      onSaved()
    } catch (failure) {
      setError(describeError(failure).message)
    } finally {
      setSaving(false)
    }
  }

  const loadingParts = Boolean(spaceId) && (collections.isPending || pages.isPending)
  return (
    <Dialog
      description="A module bundles parts you already have and gives them a place in the sidebar. Switching it off hides them; nothing is deleted."
      dirty={dirty}
      footer={
        <>
          <Button onClick={onClose} variant="ghost">
            Cancel
          </Button>
          <Button busy={saving} type="submit" variant="primary">
            {editing ? 'Save' : 'Build'}
          </Button>
        </>
      }
      onClose={onClose}
      onSubmit={(event) => void save(event)}
      open
      title={editing ? `Edit ${opened.name}` : 'Build a module'}
      width={560}
    >
      {error ? <Banner tone="danger">{error}</Banner> : null}
      <TextField
        autoFocus
        label="Name"
        maxLength={60}
        onChange={(event) => {
          setDirty(true)
          setName(event.target.value)
        }}
        required
        value={name}
      />
      {editing ? null : (
        <TextField
          hint="How agents and packages name it. Lowercase letters, digits and underscores."
          label="Key"
          maxLength={40}
          mono
          onChange={(event) => setKey(event.target.value)}
          pattern="[a-z][a-z0-9_]*"
          value={shownKey}
        />
      )}
      <TextField
        label="What it’s for"
        maxLength={280}
        onChange={(event) => {
          setDirty(true)
          setDescription(event.target.value)
        }}
        value={description}
      />
      <div className="aw-builder__pair">
        <SelectField label="Icon" onChange={(event) => setIcon(event.target.value as IconName)} prefix={<Icon name={icon} size={16} />} value={icon}>
          {MODULE_ICONS.map((choice) => (
            <option key={choice.value} value={choice.value}>
              {choice.label}
            </option>
          ))}
        </SelectField>
        {editing ? null : (
          <SelectField
            label="Space"
            onChange={(event) => {
              setSpaceId(event.target.value)
              setPicked({})
            }}
            value={spaceId}
          >
            {projects.map((project) => (
              <option key={project.id} value={project.id}>
                {project.name}
              </option>
            ))}
          </SelectField>
        )}
      </div>
      <fieldset className="aw-builder__parts">
        <legend className="aw-field__label">Parts</legend>
        {loadingParts ? (
          <ListSkeleton label="Loading the space’s parts" rows={2} />
        ) : choices.length === 0 ? (
          <p className="aw-quiet">This space has no collections or pages yet.</p>
        ) : (
          <ul className="aw-builder__list">
            {choices.map((part) => {
              const state = picked[partKey(part)] ?? { inModule: false, inSidebar: false }
              return (
                <li className="aw-builder__part" key={partKey(part)}>
                  <Checkbox
                    checked={state.inModule}
                    description={part.kind === 'collection' ? 'Collection' : 'Page'}
                    label={part.name}
                    onChange={(inModule) => change(part, { inModule })}
                  />
                  {state.inModule ? (
                    <Checkbox checked={state.inSidebar} label="In the sidebar" onChange={(inSidebar) => change(part, { inSidebar })} />
                  ) : null}
                </li>
              )
            })}
          </ul>
        )}
      </fieldset>
    </Dialog>
  )
}
