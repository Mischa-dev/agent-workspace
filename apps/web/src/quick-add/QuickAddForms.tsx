import { useMemo, useState, type FormEvent } from 'react'
import { useMutation, useQuery } from '@tanstack/react-query'
import { useNavigate } from 'react-router-dom'

import { queryKeys } from '../api/queryKeys'
import { isConnectionError, readLocal, writeLocal } from '../app/localDrafts'
import { useRetryWhenOnline } from '../app/useRetryWhenOnline'
import { useWorkspaceId } from '../app/workspace'
import { getProjectConversationSidebar, sendConversationMessage } from '../conversations/api'
import { BLANK_MESSAGE_CONTENT, buildMessageContent, MAX_MESSAGE_SCALAR_VALUES } from '../conversations/composerModel'
import type { ConversationKind } from '../conversations/types'
import { conversationTitle, openDm, useDirectOverview } from '../direct/api'
import { saveForLater } from '../later/api'
import { readRecentPlaces } from '../shell/recentPlaces'
import type { Project } from '../projects/types'
import type { FileAttachment } from '../files/api'
import { Banner, Button, ButtonLink, SelectField, TextArea, TextField, describeError } from '../ui'
import { useCreateWorkItem } from '../work-items/hooks'
import { useTaskDraft } from '../work-items/taskDraft'
import { messageFromShare, taskFromShare, type SharedContent } from './sharedContent'
import { uploadShared } from './sharedFiles'
import '../styles/pages.css'
import { here, spaceHref } from '../app/href'

const LAST_PROJECT = 'quick-add:project'
/** One draft for quick add, whichever project it's going to, so changing the project keeps what's typed. */
const QUICK_DRAFT = 'quick-add'
const LAST_PLACE = 'quick-share:place'
const DIRECT = 'direct'
const NO_FILES: File[] = []

function OfflineNote({ what }: { what: string }) {
  return (
    <Banner tone="offline">
      No connection. Your {what} is saved on this device and goes out by itself when you’re back online.
    </Banner>
  )
}

export function QuickTaskForm({ files = NO_FILES, projects, shared }: { files?: File[]; projects: Project[]; shared: SharedContent | null }) {
  const navigate = useNavigate()
  const create = useCreateWorkItem()
  const [chosen, setChosen] = useState(() => readLocal<string>(LAST_PROJECT) ?? '')
  const projectId = projects.some((project) => project.id === chosen) ? chosen : (projects[0]?.id ?? '')
  const start = useMemo(
    () => (shared ? taskFromShare(shared) : files.length ? { title: files[0].name.slice(0, 240), description: '' } : undefined),
    [files, shared],
  )
  const { draft, update, clear } = useTaskDraft(QUICK_DRAFT, start)
  const [offline, setOffline] = useState(false)
  const [attaching, setAttaching] = useState(false)
  // A task made whose files didn't go up: it's kept here so it can be opened, not made twice.
  const [stranded, setStranded] = useState<{ key: string; href: string; reason: string } | null>(null)

  // The text moves with the project; a fresh request key goes with it, so it can't land in the old one.
  const changeProject = (next: string) => {
    const { title, description } = draft
    clear()
    update({ title, description })
    setChosen(next)
  }

  const send = () => {
    if (!projectId || !draft.title.trim() || create.isPending || attaching) return
    setStranded(null)
    create.mutate(
      {
        projectId,
        title: draft.title.trim(),
        description: draft.description,
        priority: 'no_priority',
        idempotencyKey: draft.idempotencyKey,
      },
      {
        onSuccess: (item) => {
          setOffline(false)
          clear()
          writeLocal(LAST_PROJECT, projectId)
          const href = spaceHref(item.projectId, `/work-items/${encodeURIComponent(item.id)}`)
          if (!files.length) {
            navigate(href, { replace: true })
            return
          }
          // The task exists; its files go up next. If they don't, the task is one tap away.
          setAttaching(true)
          uploadShared(files, { kind: 'task', projectId: item.projectId, workItemId: item.id })
            .then(() => navigate(href, { replace: true }))
            .catch((reason: unknown) => setStranded({ key: item.key, href, reason: describeError(reason).message }))
            .finally(() => setAttaching(false))
        },
        onError: (reason) => setOffline(isConnectionError(reason)),
      },
    )
  }
  useRetryWhenOnline(offline, send)

  const submit = (event: FormEvent<HTMLFormElement>) => {
    event.preventDefault()
    send()
  }

  return (
    <form className="aw-form aw-card aw-formcard aw-quick__form" noValidate onSubmit={submit}>
      <SelectField id="quick-task-project" label="Project" onChange={(event) => changeProject(event.target.value)} value={projectId}>
        {projects.map((project) => (
          <option key={project.id} value={project.id}>
            {project.name}
          </option>
        ))}
      </SelectField>
      <TextField
        autoFocus={!shared}
        enterKeyHint="done"
        id="quick-task-title"
        label="Title"
        maxLength={240}
        onChange={(event) => update({ title: event.target.value })}
        placeholder="What needs doing?"
        required
        value={draft.title}
      />
      <TextArea
        id="quick-task-description"
        label="Description"
        maxLength={100000}
        onChange={(event) => update({ description: event.target.value })}
        placeholder="Links, details, what done looks like"
        rows={4}
        value={draft.description}
      />
      {offline ? (
        <OfflineNote what="task" />
      ) : create.isError ? (
        <Banner tone="danger">The task wasn’t created. {describeError(create.error).message}</Banner>
      ) : null}
      {attaching ? <Banner tone="info">Attaching files…</Banner> : null}
      {stranded ? (
        <Banner
          action={
            <ButtonLink size="sm" to={stranded.href} variant="primary">
              Open {stranded.key}
            </ButtonLink>
          }
          tone="danger"
        >
          {stranded.key} was created, but the files weren’t attached. {stranded.reason} Attach them from the task.
        </Banner>
      ) : null}
      <div className="aw-quick__actions">
        <Button
          busy={create.isPending || attaching}
          disabled={!draft.title.trim() || !projectId}
          icon="plus"
          size="lg"
          type="submit"
          variant="primary"
        >
          Create task
        </Button>
      </div>
    </form>
  )
}

type Place = { id: string; kind: ConversationKind; label: string; href: string }

export function QuickMessageForm({
  files = NO_FILES,
  projects,
  shared,
  viewerId,
}: {
  files?: File[]
  projects: Project[]
  shared: SharedContent | null
  viewerId: string
}) {
  const navigate = useNavigate()
  const [where, setWhere] = useState(() => readLocal<string>(LAST_PLACE) ?? DIRECT)
  const scope = where === DIRECT || projects.some((project) => project.id === where) ? where : DIRECT
  const direct = useDirectOverview()
  const sidebar = useQuery({
    queryKey: queryKeys.conversationSidebar(scope),
    queryFn: () => getProjectConversationSidebar(scope),
    enabled: scope !== DIRECT,
  })
  const places = useMemo<Place[]>(() => {
    // Send to…: the conversations you opened lately come first.
    const recent = readRecentPlaces(viewerId).map((place) => place.path)
    const rank = (place: Place) => {
      const at = recent.findIndex((path) => path === place.href || path.endsWith(place.href))
      return at < 0 ? recent.length : at
    }
    const byRecent = (list: Place[]) =>
      list
        .map((place, index) => ({ place, index }))
        .sort((left, right) => rank(left.place) - rank(right.place) || left.index - right.index)
        .map(({ place }) => place)
    if (scope === DIRECT) {
      return byRecent((direct.data?.conversations ?? []).map((conversation) => ({
        id: conversation.id,
        kind: conversation.kind,
        label: conversationTitle(conversation, viewerId) || 'Conversation',
        href: here(`/messages/${encodeURIComponent(conversation.id)}`),
      })))
    }
    return byRecent(
      (sidebar.data?.channels ?? [])
        .filter((channel) => !channel.archivedAt)
        .map((channel) => ({
          id: channel.id,
          kind: 'channel' as const,
          label: `#${channel.name}`,
          href: spaceHref(scope, `/channels/${encodeURIComponent(channel.id)}`),
        })),
    )
  }, [direct.data, scope, sidebar.data, viewerId])
  const [placeId, setPlaceId] = useState('')
  const place = places.find((candidate) => candidate.id === placeId) ?? places[0]
  const [text, setText] = useState(() => (shared ? messageFromShare(shared) : ''))
  const [idempotencyKey, setIdempotencyKey] = useState(() => crypto.randomUUID())
  const [offline, setOffline] = useState(false)
  const loading = scope === DIRECT ? direct.isPending : sidebar.isPending
  const loadError = scope === DIRECT ? direct.error : sidebar.error
  const typed = [...text.trim()].length <= MAX_MESSAGE_SCALAR_VALUES ? buildMessageContent(text, []) : null
  const content = typed ?? (files.length && !text.trim() ? BLANK_MESSAGE_CONTENT : null)
  // Uploaded once per place, so a retry after a dropped connection doesn't upload twice.
  const [uploaded, setUploaded] = useState<{ placeId: string; attachments: FileAttachment[] } | null>(null)

  const post = useMutation({
    mutationFn: async (target: Place) => {
      if (!content) throw new Error('Write something to post.')
      let attachments = uploaded?.placeId === target.id ? uploaded.attachments : []
      if (files.length && !attachments.length) {
        attachments = await uploadShared(files, { kind: 'conversation', conversationId: target.id })
        setUploaded({ placeId: target.id, attachments })
      }
      return sendConversationMessage({ conversationId: target.id, kind: target.kind, content, idempotencyKey, attachments })
    },
    onSuccess: (_response, target) => {
      setOffline(false)
      setIdempotencyKey(crypto.randomUUID())
      writeLocal(LAST_PLACE, scope)
      navigate(target.href, { replace: true })
    },
    onError: (reason) => setOffline(isConnectionError(reason)),
  })
  const send = () => {
    if (place && content && !post.isPending) post.mutate(place)
  }
  useRetryWhenOnline(offline, send)

  const submit = (event: FormEvent<HTMLFormElement>) => {
    event.preventDefault()
    send()
  }

  return (
    <form className="aw-form aw-card aw-formcard aw-quick__form" noValidate onSubmit={submit}>
      <SelectField
        id="quick-share-where"
        label="Where"
        onChange={(event) => {
          setWhere(event.target.value)
          setPlaceId('')
        }}
        value={scope}
      >
        <option value={DIRECT}>Direct messages</option>
        {projects.map((project) => (
          <option key={project.id} value={project.id}>
            {project.name}
          </option>
        ))}
      </SelectField>
      <SelectField
        disabled={loading || places.length === 0}
        hint={
          loadError
            ? loadError.message
            : !loading && places.length === 0
              ? scope === DIRECT
                ? 'No direct messages yet. Start one from Messages.'
                : 'This project has no channels you can post in.'
              : undefined
        }
        id="quick-share-conversation"
        label="Conversation"
        onChange={(event) => setPlaceId(event.target.value)}
        value={place?.id ?? ''}
      >
        {loading ? <option value="">Loading…</option> : null}
        {places.map((candidate) => (
          <option key={candidate.id} value={candidate.id}>
            {candidate.label}
          </option>
        ))}
      </SelectField>
      <TextArea
        id="quick-share-text"
        label="Message"
        onChange={(event) => setText(event.target.value)}
        placeholder="Say something about it"
        rows={5}
        value={text}
      />
      {offline ? (
        <OfflineNote what="message" />
      ) : post.isError ? (
        <Banner tone="danger">The message wasn’t posted. {describeError(post.error).message}</Banner>
      ) : null}
      <div className="aw-quick__actions">
        <Button busy={post.isPending} disabled={!place || !content} icon="send" size="lg" type="submit" variant="primary">
          Post
        </Button>
      </div>
    </form>
  )
}

/** The most a Later note holds. */
const MAX_NOTE = 1000

/**
 * Capture for yourself (C2.32): a note to self (your DM with yourself, files
 * and all), or, when something was shared, a note in Later.
 */
export function QuickNoteForm({
  files = NO_FILES,
  mode,
  shared,
  viewerId,
}: {
  files?: File[]
  mode: 'note' | 'later'
  shared: SharedContent | null
  viewerId: string
}) {
  const navigate = useNavigate()
  const workspaceId = useWorkspaceId()
  const [text, setText] = useState(() => (shared ? messageFromShare(shared) : ''))
  const [idempotencyKey, setIdempotencyKey] = useState(() => crypto.randomUUID())
  const [offline, setOffline] = useState(false)
  const [uploaded, setUploaded] = useState<{ conversationId: string; attachments: FileAttachment[] } | null>(null)
  const typed = [...text.trim()].length <= MAX_MESSAGE_SCALAR_VALUES ? buildMessageContent(text, []) : null
  const content = typed ?? (files.length && !text.trim() ? BLANK_MESSAGE_CONTENT : null)
  const note = text.trim()
  const ready = mode === 'later' ? note.length > 0 && [...note].length <= MAX_NOTE : Boolean(content)

  const capture = useMutation({
    mutationFn: async () => {
      if (mode === 'later') {
        await saveForLater({ note }, workspaceId)
        return here('/later')
      }
      if (!content) throw new Error('Write something to keep.')
      const conversationId = await openDm(viewerId)
      let attachments = uploaded?.conversationId === conversationId ? uploaded.attachments : []
      if (files.length && !attachments.length) {
        attachments = await uploadShared(files, { kind: 'conversation', conversationId })
        setUploaded({ conversationId, attachments })
      }
      await sendConversationMessage({ conversationId, kind: 'dm', content, idempotencyKey, attachments })
      return here(`/messages/${encodeURIComponent(conversationId)}`)
    },
    onSuccess: (href) => {
      setOffline(false)
      setIdempotencyKey(crypto.randomUUID())
      navigate(href, { replace: true })
    },
    onError: (reason) => setOffline(isConnectionError(reason)),
  })
  const send = () => {
    if (ready && !capture.isPending) capture.mutate()
  }
  useRetryWhenOnline(offline, send)

  const submit = (event: FormEvent<HTMLFormElement>) => {
    event.preventDefault()
    send()
  }

  return (
    <form className="aw-form aw-card aw-formcard aw-quick__form" noValidate onSubmit={submit}>
      <TextArea
        hint={
          mode === 'later'
            ? files.length
              ? 'Later keeps the note, not the files: a note to self keeps both.'
              : 'It waits in Later until you mark it done.'
            : 'Only you see your notes to self.'
        }
        id="quick-note-text"
        label={mode === 'later' ? 'Note' : 'Note to self'}
        maxLength={mode === 'later' ? MAX_NOTE : undefined}
        onChange={(event) => setText(event.target.value)}
        placeholder={mode === 'later' ? 'What to come back to' : 'Write it down'}
        rows={5}
        value={text}
      />
      {offline ? (
        <OfflineNote what={mode === 'later' ? 'note' : 'note to self'} />
      ) : capture.isError ? (
        <Banner tone="danger">It wasn’t kept. {describeError(capture.error).message}</Banner>
      ) : null}
      <div className="aw-quick__actions">
        <Button busy={capture.isPending} disabled={!ready} icon={mode === 'later' ? 'bookmark' : 'send'} size="lg" type="submit" variant="primary">
          {mode === 'later' ? 'Save to Later' : 'Keep note'}
        </Button>
      </div>
    </form>
  )
}
