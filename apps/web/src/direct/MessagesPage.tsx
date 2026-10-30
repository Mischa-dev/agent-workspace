import { useEffect, useId, useMemo, useRef, useState, type KeyboardEvent } from 'react'
import { useQueryClient } from '@tanstack/react-query'
import { useNavigate, useSearchParams } from 'react-router-dom'

import { readLocal, writeLocal } from '../app/localDrafts'
import { useAuthenticatedSession } from '../auth/session'
import { sendConversationMessage } from '../conversations/api'
import { ConversationHeader } from '../conversations/ConversationHeader'
import type { ComposerDraft, ComposerSubmission } from '../conversations/composerModel'
import { StructuredComposer } from '../conversations/StructuredComposer'
import { PendingMessages } from '../conversations/Timeline'
import type { ConversationActor } from '../conversations/types'
import { ExternalAgentSettings } from '../external-agents/ExternalAgentSettings'
import { useProjects } from '../projects/hooks'
import { AppShell } from '../shell/AppShell'
import {
  ActorTag,
  Avatar,
  Banner,
  ButtonLink,
  ConversationIntro,
  EmptyState,
  ErrorState,
  Icon,
  ListSkeleton,
  PageHeader,
  SelectField,
  TextField,
  actorKind,
  describeError,
} from '../ui'
import { createGroup, directQueryKey, openDm, useDirectOverview } from './api'

function listNames(names: string[]) {
  if (names.length <= 1) return names.join('')
  return `${names.slice(0, -1).join(', ')} and ${names[names.length - 1]}`
}

/**
 * New message, as an empty chat: pick who it's to in the To bar, type in the
 * box at the bottom, and sending starts the DM (or the group) with that first
 * message. An existing DM is reused.
 */
export function MessagesPage() {
  const session = useAuthenticatedSession()
  const overview = useDirectOverview()
  const queryClient = useQueryClient()
  const navigate = useNavigate()
  const [searchParams] = useSearchParams()
  const listId = useId()
  const toInput = useRef<HTMLInputElement>(null)
  const groupKey = useRef(crypto.randomUUID())
  // A conversation made by a send whose message then failed: sending again goes there.
  const started = useRef<{ key: string; conversationId: string } | null>(null)
  const [selected, setSelected] = useState<string[]>(() => {
    const to = searchParams.get('to')
    return to ? [to] : []
  })
  const [query, setQuery] = useState('')
  const [listOpen, setListOpen] = useState(false)
  const [active, setActive] = useState(0)
  // Arrowed to a name: Enter picks it even with nothing typed.
  const [browsing, setBrowsing] = useState(false)
  const [groupName, setGroupName] = useState('')
  // The first message while it starts the conversation, shown dimmed as in any chat.
  const [sending, setSending] = useState<ComposerSubmission[]>([])
  const people = useMemo(() => overview.data?.people ?? [], [overview.data?.people])
  const chosen = selected.map((id) => people.find((person) => person.id === id)).filter((person): person is ConversationActor => Boolean(person))
  const needle = query.trim().toLowerCase()
  const matches = people.filter(
    (person) =>
      !selected.includes(person.id) &&
      (!needle || person.displayName.toLowerCase().includes(needle) || person.handle.toLowerCase().includes(needle)),
  )
  const activeIndex = Math.min(active, Math.max(matches.length - 1, 0))
  const existing =
    chosen.length === 1
      ? overview.data?.conversations.find(
          (conversation) => conversation.kind === 'dm' && conversation.members.some((member) => member.id === chosen[0].id),
        )
      : undefined

  // Ready to type a name, with a mouse or trackpad; on a touch screen it would pop the keyboard up.
  const ready = Boolean(overview.data && people.length)
  useEffect(() => {
    if (ready && window.matchMedia?.('(pointer: fine)').matches) toInput.current?.focus()
  }, [ready])

  const changeSelection = (next: string[]) => {
    setSelected(next)
    groupKey.current = crypto.randomUUID()
  }
  const pick = (person: ConversationActor) => {
    changeSelection([...selected, person.id])
    setQuery('')
    setActive(0)
    setBrowsing(false)
    toInput.current?.focus()
  }
  const unpick = (id: string) => {
    changeSelection(selected.filter((candidate) => candidate !== id))
    toInput.current?.focus()
  }

  const onToKeyDown = (event: KeyboardEvent<HTMLInputElement>) => {
    if (event.key === 'ArrowDown' || event.key === 'ArrowUp') {
      event.preventDefault()
      setListOpen(true)
      setBrowsing(true)
      if (!matches.length) return
      const step = event.key === 'ArrowDown' ? 1 : -1
      setActive((activeIndex + step + matches.length) % matches.length)
    } else if (event.key === 'Enter') {
      event.preventDefault()
      // With nothing typed, Enter goes on to the message once someone is picked.
      const match = needle || browsing || !chosen.length ? matches[activeIndex] : undefined
      if (match) pick(match)
      else if (chosen.length) document.querySelector<HTMLTextAreaElement>('.aw-newchat__composer textarea')?.focus()
    } else if (event.key === 'Backspace' && !query && selected.length) {
      unpick(selected[selected.length - 1])
    } else if (event.key === 'Escape' && listOpen) {
      event.preventDefault()
      setListOpen(false)
    }
  }

  const send = async (submission: ComposerSubmission) => {
    if (!chosen.length) throw new Error('Add someone in To first.')
    setSending((current) => [...current, submission])
    try {
      await start(submission)
    } finally {
      setSending((current) => current.filter((item) => item.idempotencyKey !== submission.idempotencyKey))
    }
  }

  const start = async (submission: ComposerSubmission) => {
    const isGroup = chosen.length > 1
    const memberIds = chosen.map((person) => person.id)
    const key = isGroup ? `group:${[...memberIds].sort().join(',')}:${groupName.trim()}` : `dm:${memberIds[0]}`
    let conversationId = started.current?.key === key ? started.current.conversationId : null
    if (!conversationId) {
      try {
        conversationId = isGroup
          ? await createGroup({ name: groupName, memberIds, idempotencyKey: groupKey.current })
          : await openDm(memberIds[0])
      } catch (reason) {
        throw new Error(`The conversation didn’t start. ${describeError(reason).message}`, { cause: reason })
      }
      started.current = { key, conversationId }
    }
    await sendConversationMessage({ conversationId, kind: isGroup ? 'group' : 'dm', ...submission })
    // `started` stays set until the page goes: a second message sent before then joins the same conversation.
    await queryClient.invalidateQueries({ queryKey: directQueryKey })
    // Anything typed while the first message went out becomes the conversation's draft, so leaving here keeps it.
    const followUp = document.querySelector<HTMLTextAreaElement>('.aw-newchat__composer textarea')?.value ?? ''
    if (followUp.trim()) {
      // After any draft already waiting there, never over it.
      const waiting = readLocal<ComposerDraft>(`draft:${conversationId}`)
      writeLocal<ComposerDraft>(
        `draft:${conversationId}`,
        waiting?.text.trim()
          ? { ...waiting, text: `${waiting.text}\n${followUp}` }
          : { text: followUp, mentions: [], workItems: [] },
      )
    }
    navigate(`/messages/${encodeURIComponent(conversationId)}`)
  }

  const names = chosen.map((person) => person.displayName)
  const only = chosen.length === 1 ? chosen[0] : null
  const onlyKind = only ? actorKind(only.kind) : 'person'
  const showList = listOpen && (matches.length > 0 || Boolean(needle))

  return (
    <AppShell direct mainClassName="aw-main--conversation" user={session.user}>
      <ConversationHeader icon="compose" title="New message" />
      {overview.isError ? (
        <ErrorState error={overview.error} onRetry={() => void overview.refetch()} title="Messages didn’t load" />
      ) : null}
      {overview.data && people.length > 0 ? (
        <div className="aw-newchat__to">
          <label className="aw-newchat__label" htmlFor={`${listId}-input`}>
            To
          </label>
          <div className="aw-newchat__field">
            {chosen.map((person) => (
              <button
                aria-label={`Remove ${person.displayName}`}
                className="aw-newchat__chip"
                key={person.id}
                onClick={() => unpick(person.id)}
                type="button"
              >
                <Avatar kind={actorKind(person.kind)} name={person.displayName} seed={person.id} size={20} />
                {person.displayName}
                <Icon name="x" size={14} />
              </button>
            ))}
            <input
              aria-activedescendant={showList && matches[activeIndex] ? `${listId}-${matches[activeIndex].id}` : undefined}
              aria-autocomplete="list"
              aria-controls={`${listId}-list`}
              aria-expanded={showList}
              autoComplete="off"
              className="aw-newchat__input"
              id={`${listId}-input`}
              onBlur={() => setListOpen(false)}
              onChange={(event) => {
                setQuery(event.target.value)
                setActive(0)
                setListOpen(true)
              }}
              onFocus={() => setListOpen(true)}
              onKeyDown={onToKeyDown}
              placeholder={chosen.length ? 'Add someone else' : 'Type a name'}
              ref={toInput}
              role="combobox"
              spellCheck={false}
              value={query}
            />
          </div>
          {showList ? (
            <ul aria-label="People and agents" className="aw-newchat__list" id={`${listId}-list`} role="listbox">
              {matches.length === 0 ? (
                <li className="aw-newchat__none" role="presentation">
                  No one matches “{query.trim()}”.
                </li>
              ) : null}
              {matches.map((person, index) => {
                const kind = actorKind(person.kind)
                return (
                  <li
                    aria-selected={index === activeIndex}
                    className="aw-newchat__option"
                    id={`${listId}-${person.id}`}
                    key={person.id}
                    onClick={() => pick(person)}
                    onMouseDown={(event) => event.preventDefault()}
                    onMouseEnter={() => setActive(index)}
                    role="option"
                  >
                    <Avatar kind={kind} name={person.displayName} seed={person.id} size={28} />
                    <span className="aw-newchat__name">{person.displayName}</span>
                    <ActorTag kind={kind} />
                    <span className="aw-newchat__handle">@{person.handle}</span>
                  </li>
                )
              })}
            </ul>
          ) : null}
        </div>
      ) : null}
      <div className="aw-newchat__body">
        {overview.isPending ? <ListSkeleton label="Loading people" rows={2} avatars /> : null}
        {overview.data && people.length === 0 ? (
          <EmptyState
            action={
              overview.data.capabilities.manageAgents ? (
                <ButtonLink icon="bot" to="/messages/agents" variant="primary">
                  Add an agent
                </ButtonLink>
              ) : undefined
            }
            icon="users"
            title="No one to message yet"
          >
            {overview.data.capabilities.manageAgents
              ? 'Add an agent to start. Each one gets its own identity and shows up here.'
              : 'Ask your workspace owner to add an agent. Each one gets its own identity and shows up here.'}
          </EmptyState>
        ) : null}
        {overview.data && people.length > 0 ? (
          only ? (
            <ConversationIntro
              actions={
                existing ? (
                  <ButtonLink to={`/messages/${encodeURIComponent(existing.id)}`} variant="ghost">
                    Open the conversation
                  </ButtonLink>
                ) : undefined
              }
              mark={<Avatar kind={onlyKind} name={only.displayName} seed={only.id} size={64} />}
              title={
                <>
                  {only.displayName}
                  <ActorTag kind={onlyKind} />
                </>
              }
            >
              {existing
                ? `You already have a conversation with ${only.displayName}. Your message goes there.`
                : onlyKind === 'agent'
                  ? `Your first message starts the conversation and asks ${only.displayName} for a reply.`
                  : `Your first message starts your conversation with ${only.displayName}. Only the two of you can see it.`}
            </ConversationIntro>
          ) : chosen.length > 1 ? (
            <ConversationIntro icon="users" title={`A group with ${listNames(names)}`}>
              Your first message starts the group. Agents answer when they’re asked or mentioned.
            </ConversationIntro>
          ) : (
            <ConversationIntro icon="compose" title="New message">
              Choose who it’s to above: one person or agent for a direct message, or several for a group.
            </ConversationIntro>
          )
        ) : null}
        <PendingMessages entries={[]} items={sending} viewer={{ actorId: session.user.actorId, displayName: session.user.displayName }} />
        {chosen.length > 1 ? (
          <div className="aw-newchat__group">
            <TextField
              hint="Leave it blank to name the group after its members."
              id="group-name"
              label="Group name (optional)"
              maxLength={120}
              onChange={(event) => {
                setGroupName(event.target.value)
                groupKey.current = crypto.randomUUID()
              }}
              value={groupName}
            />
          </div>
        ) : null}
      </div>
      {overview.data && people.length > 0 ? (
        <StructuredComposer
          className="aw-newchat__composer"
          label={only ? `Message ${only.displayName}` : chosen.length ? `Message ${listNames(names)}` : 'Message'}
          onSubmit={send}
          placeholder={only ? `Message ${only.displayName}` : chosen.length ? `Message ${listNames(names)}` : 'Write a message'}
          searchActors={async (text) => {
            const lowered = text.trim().toLowerCase()
            return chosen.filter(
              (person) => !lowered || person.handle.toLowerCase().includes(lowered) || person.displayName.toLowerCase().includes(lowered),
            )
          }}
        />
      ) : null}
    </AppShell>
  )
}

/** Add agents and choose where each works. Every agent can be messaged; a project adds its tasks and channels. */
export function AgentsPage() {
  const session = useAuthenticatedSession()
  const overview = useDirectOverview()
  const projects = useProjects()
  const queryClient = useQueryClient()
  const [place, setPlace] = useState<string | null>(null)
  // New agents should appear in the people list as soon as you leave here.
  useEffect(() => () => void queryClient.invalidateQueries({ queryKey: directQueryKey }), [queryClient])
  const spaceId = overview.data?.projectId ?? ''
  const places = [
    { id: spaceId, name: 'Messages only' },
    ...(projects.data?.projects ?? [])
      .filter((project) => project.organizationId === overview.data?.organizationId && !project.archivedAt && project.capabilities.manage)
      .map((project) => ({ id: project.id, name: project.name })),
  ]
  const shown = places.some((candidate) => candidate.id === place) ? (place ?? spaceId) : spaceId
  return (
    <AppShell direct mainClassName="aw-main--page" user={session.user}>
      <div className="aw-page">
        <PageHeader icon="bot" title="Agents">
          Every agent you add can be messaged directly. Choose a project when it should also work there, in its tasks and
          channels.
        </PageHeader>
        {overview.data?.capabilities.manageAgents ? (
          <>
            {places.length > 1 ? (
              <div className="aw-agentplace">
                <SelectField
                  hint="The agents listed below, and where a new one starts."
                  id="agent-place"
                  label="Where they work"
                  onChange={(event) => setPlace(event.target.value)}
                  value={shown}
                >
                  {places.map((candidate) => (
                    <option key={candidate.id} value={candidate.id}>
                      {candidate.name}
                    </option>
                  ))}
                </SelectField>
              </div>
            ) : null}
            <ExternalAgentSettings key={shown} projectId={shown} />
          </>
        ) : overview.data ? (
          <Banner tone="info">Only the workspace owner can add and manage agents.</Banner>
        ) : overview.isPending ? (
          <ListSkeleton label="Loading agents" rows={2} avatars />
        ) : null}
      </div>
    </AppShell>
  )
}
