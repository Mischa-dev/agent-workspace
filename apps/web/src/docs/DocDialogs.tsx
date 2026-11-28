import { useRef, useState } from 'react'
import { useQuery, useQueryClient } from '@tanstack/react-query'

import type { DocEntry } from '../api/generated/operations'
import { callOp, invalidateOps, newRequestId, opQuery, ranOrThrow, useOp } from '../api/ops'
import { Banner, Button, Checkbox, Dialog, ListSkeleton, SegmentedControl, SelectField, TextField, describeError, showToast } from '../ui'
import { docKeys, folderChoices, type MadeKind } from './docsModel'

const TOP = ''

function FolderField({
  entries,
  label,
  moving,
  onChange,
  value,
}: {
  entries: readonly DocEntry[]
  label: string
  moving?: string
  onChange: (folderId: string) => void
  value: string
}) {
  const choices = folderChoices(entries, moving)
  if (!choices.length && !value) return null
  return (
    <SelectField label={label} onChange={(event) => onChange(event.target.value)} value={value}>
      <option value={TOP}>Top of Docs</option>
      {choices.map((choice) => (
        <option key={choice.id} value={choice.id}>
          {choice.label}
        </option>
      ))}
    </SelectField>
  )
}

/** Makes one document of `kind` and returns its id. */
async function make(kind: MadeKind, projectId: string, name: string, requestId: string): Promise<string> {
  switch (kind) {
    case 'page':
      return ranOrThrow(await callOp('page_create', { projectId, title: name, body: '' }, { requestId }), 'Making it').id
    case 'sheet':
      return ranOrThrow(await callOp('sheet_create', { projectId, name }, { requestId }), 'Making it').id
    case 'whiteboard':
      return ranOrThrow(await callOp('whiteboard_create', { projectId, title: name }, { requestId }), 'Making it').whiteboard.id
    case 'collection':
      return ranOrThrow(await callOp('collection_create', { projectId, name, description: '', titleName: 'Name' }, { requestId }), 'Making it').collection.id
    case 'dashboard':
      return ranOrThrow(await callOp('dashboard_create', { spaceId: projectId, name, shared: true }, { requestId }), 'Making it').id
  }
}

/** A new page, sheet, whiteboard, collection, dashboard or folder, made where the person is in the tree. */
export function NewDocDialog({
  entries,
  folderId,
  kind,
  noun,
  onClose,
  onMade,
  projectId,
}: {
  entries: readonly DocEntry[]
  folderId: string | null
  kind: MadeKind | 'folder'
  /** What the layout calls this kind, in lower case. */
  noun: string
  onClose: () => void
  onMade: (made: { id: string; kind: MadeKind | 'folder'; folderId: string | null }) => void
  projectId: string
}) {
  const queryClient = useQueryClient()
  const [name, setName] = useState('')
  const [folder, setFolder] = useState(folderId ?? TOP)
  const [problem, setProblem] = useState<string | null>(null)
  const [busy, setBusy] = useState(false)
  // One request id for the dialog: a second press after a dropped answer finds what the first made.
  const requestId = useRef(newRequestId())
  const made = useRef<string | null>(null)
  const submit = async () => {
    const title = name.trim()
    if (!title) return setProblem(`Give the ${noun} a name.`)
    setProblem(null)
    setBusy(true)
    try {
      if (kind === 'folder') {
        const created = ranOrThrow(await callOp('doc_folder_create', { spaceId: projectId, title, ...(folder ? { parentFolderId: folder } : {}) }, { requestId: requestId.current }), 'Making it')
        made.current = created.id
      } else {
        made.current ??= await make(kind, projectId, title, requestId.current)
        if (folder) ranOrThrow(await callOp('doc_move', { id: made.current, folderId: folder }), 'Putting it in the folder')
      }
      await invalidateOps(queryClient, ['doc_list'])
      onMade({ id: made.current, kind, folderId: folder || null })
      onClose()
    } catch (error) {
      setProblem(
        made.current && kind !== 'folder'
          ? `The ${noun} was made at the top of Docs, but it wasn’t put in the folder. ${describeError(error).message}`
          : describeError(error).message,
      )
      if (made.current) void invalidateOps(queryClient, ['doc_list'])
      setBusy(false)
    }
  }

  return (
    <Dialog
      dirty
      footer={
        <>
          <Button onClick={onClose} variant="ghost">
            Cancel
          </Button>
          <Button busy={busy} type="submit" variant="primary">
            {`Make ${noun}`}
          </Button>
        </>
      }
      onClose={onClose}
      onSubmit={(event) => {
        event.preventDefault()
        void submit()
      }}
      open
      title={`New ${noun}`}
    >
      {problem ? <Banner tone="danger">{problem}</Banner> : null}
      <TextField autoFocus label="Name" maxLength={120} onChange={(event) => setName(event.target.value)} required value={name} />
      <FolderField entries={entries} label="Folder" onChange={setFolder} value={folder} />
    </Dialog>
  )
}

export function RenameFolderDialog({ folder, onClose }: { folder: DocEntry; onClose: () => void }) {
  const [title, setTitle] = useState(folder.title)
  const [problem, setProblem] = useState<string | null>(null)
  const rename = useOp('doc_folder_update', { invalidates: ['doc_list'] })
  const submit = async () => {
    if (!title.trim()) return setProblem('Give the folder a name.')
    setProblem(null)
    try {
      ranOrThrow(await rename.mutateAsync({ folderId: folder.id, title: title.trim() }), 'Renaming it')
      onClose()
    } catch (error) {
      setProblem(describeError(error).message)
    }
  }
  return (
    <Dialog
      dirty
      footer={
        <>
          <Button onClick={onClose} variant="ghost">
            Cancel
          </Button>
          <Button busy={rename.isPending} type="submit" variant="primary">
            Rename
          </Button>
        </>
      }
      onClose={onClose}
      onSubmit={(event) => {
        event.preventDefault()
        void submit()
      }}
      open
      title="Rename folder"
    >
      {problem ? <Banner tone="danger">{problem}</Banner> : null}
      <TextField autoFocus label="Name" maxLength={120} onChange={(event) => setTitle(event.target.value)} required value={title} />
    </Dialog>
  )
}

/** Puts a document or folder in another folder, or back at the top. */
export function MoveDocDialog({ entries, entry, onClose }: { entries: readonly DocEntry[]; entry: DocEntry; onClose: () => void }) {
  const [folder, setFolder] = useState(entry.parentId ?? TOP)
  const [problem, setProblem] = useState<string | null>(null)
  const move = useOp('doc_move', { invalidates: ['doc_list', 'doc_access_get'] })
  const choices = folderChoices(entries, entry.id)
  const submit = async () => {
    setProblem(null)
    try {
      ranOrThrow(await move.mutateAsync({ id: entry.id, folderId: folder || null }), 'Moving it')
      showToast({ tone: 'success', title: folder ? `Moved to ${choices.find((choice) => choice.id === folder)?.label ?? 'the folder'}` : 'Moved to the top of Docs' })
      onClose()
    } catch (error) {
      setProblem(describeError(error).message)
    }
  }
  return (
    <Dialog
      description="It takes on who can see the folder it goes into."
      footer={
        <>
          <Button onClick={onClose} variant="ghost">
            Cancel
          </Button>
          <Button busy={move.isPending} disabled={folder === (entry.parentId ?? TOP)} type="submit" variant="primary">
            Move
          </Button>
        </>
      }
      onClose={onClose}
      onSubmit={(event) => {
        event.preventDefault()
        void submit()
      }}
      open
      title={`Move “${entry.title}”`}
    >
      {problem ? <Banner tone="danger">{problem}</Banner> : null}
      {choices.length || folder ? (
        <FolderField entries={entries} label="Move to" moving={entry.id} onChange={setFolder} value={folder} />
      ) : (
        <p className="aw-docs__note">There are no folders yet. Make one from New, then move this into it.</p>
      )}
    </Dialog>
  )
}

type Audience = 'everyone' | 'chosen'

/** Who can see one document or folder: everyone in the project, or only the people picked. */
export function DocAccessDialog({ entry, noun, onClose, projectId }: { entry: DocEntry; noun: string; onClose: () => void; projectId: string }) {
  const access = useQuery(opQuery('doc_access_get', { id: entry.id }))
  const members = useQuery({
    queryKey: docKeys.people(projectId),
    queryFn: ({ signal }) => callOp('people_search', { projectId, limit: 200 }, { signal }),
    enabled: access.data?.canChange === true,
  })
  const save = useOp('doc_access_set', { invalidates: ['doc_list', 'doc_access_get'] })
  const [audience, setAudience] = useState<Audience | null>(null)
  const [picked, setPicked] = useState<Set<string> | null>(null)
  const [problem, setProblem] = useState<string | null>(null)

  const view = access.data
  const shownAudience: Audience = audience ?? (view?.restricted ? 'chosen' : 'everyone')
  const chosen = picked ?? new Set(view?.people.map((person) => person.account))
  const above = view?.inheritedFrom
  const everyoneMeans = above
    ? `Everyone who can see ${above.title ? `“${above.title}”` : 'the folder it is in'}, which only some people can see.`
    : entry.kind === 'folder'
      ? 'Everyone in this project sees the folder and what it holds.'
      : `Everyone in this project sees the ${noun}.`

  const toggle = (account: string, on: boolean) => {
    const next = new Set(chosen)
    if (on) next.add(account)
    else next.delete(account)
    setPicked(next)
  }
  const submit = async () => {
    if (!view) return
    setProblem(null)
    try {
      const restricted = shownAudience === 'chosen'
      ranOrThrow(await save.mutateAsync({ id: entry.id, restricted, people: restricted ? [...chosen] : [] }), 'Changing who can see it')
      showToast({ tone: 'success', title: restricted ? `Only the people you chose can see “${entry.title}”` : `“${entry.title}” is open again` })
      onClose()
    } catch (error) {
      setProblem(describeError(error).message)
    }
  }

  const people = members.data?.people ?? []
  const named = new Map(view?.people.map((person) => [person.account, person.displayName]))
  // Someone named who has since left the project still shows, so they can be taken off.
  const gone = [...named].filter(([account]) => people.length > 0 && !people.some((person) => person.id === account))

  return (
    <Dialog
      footer={
        view?.canChange ? (
          <>
            <Button onClick={onClose} variant="ghost">
              Cancel
            </Button>
            <Button busy={save.isPending} type="submit" variant="primary">
              Save
            </Button>
          </>
        ) : (
          <Button onClick={onClose}>Done</Button>
        )
      }
      onClose={onClose}
      onSubmit={(event) => {
        event.preventDefault()
        if (view?.canChange) void submit()
      }}
      open
      title={`Who can see “${entry.title}”`}
    >
      {problem ? <Banner tone="danger">{problem}</Banner> : null}
      {access.isPending ? <ListSkeleton label="Loading who can see it" rows={2} /> : null}
      {access.isError ? <Banner tone="danger">{`It didn’t load. ${describeError(access.error).message}`}</Banner> : null}
      {view && !view.canChange ? (
        <div className="aw-docs__access">
          <p className="aw-docs__note">
            {view.restricted ? `Only these people can see this ${noun}. Their role still decides what they can do with it.` : everyoneMeans}
          </p>
          {view.restricted ? (
            <ul className="aw-docs__people">
              {view.people.map((person) => (
                <li key={person.account}>{person.displayName || 'Someone'}</li>
              ))}
            </ul>
          ) : null}
        </div>
      ) : null}
      {view?.canChange ? (
        <div className="aw-docs__access">
          <SegmentedControl<Audience>
            label="Who can see it"
            onChange={setAudience}
            options={[
              { value: 'everyone', label: above ? 'Same as its folder' : 'Everyone in this project' },
              { value: 'chosen', label: 'Only people I choose' },
            ]}
            value={shownAudience}
          />
          {shownAudience === 'everyone' ? (
            <p className="aw-docs__note">{everyoneMeans}</p>
          ) : (
            <>
              <p className="aw-docs__note">
                {entry.kind === 'folder'
                  ? 'Only the people ticked see this folder and what it holds. Their role still decides what they can do there.'
                  : `Only the people ticked see this ${noun}. Their role still decides what they can do with it.`}
              </p>
              {members.isPending ? <ListSkeleton label="Loading people" rows={3} /> : null}
              {members.isError ? <Banner tone="danger">{`The people of this project didn’t load. ${describeError(members.error).message}`}</Banner> : null}
              <div aria-label="People who can see it" className="aw-docs__people" role="group">
                {people.map((person) =>
                  person.id === view.you ? (
                    <Checkbox checked description="You always keep what you restrict." disabled key={person.id} label={`${person.name} (you)`} onChange={() => undefined} />
                  ) : (
                    <Checkbox
                      checked={chosen.has(person.id)}
                      description={`@${person.handle}`}
                      key={person.id}
                      label={person.name}
                      onChange={(on) => toggle(person.id, on)}
                    />
                  ),
                )}
                {gone.map(([account, name]) => (
                  <Checkbox
                    checked={chosen.has(account)}
                    description="No longer in this project."
                    key={account}
                    label={name || 'Someone'}
                    onChange={(on) => toggle(account, on)}
                  />
                ))}
              </div>
            </>
          )}
        </div>
      ) : null}
    </Dialog>
  )
}
