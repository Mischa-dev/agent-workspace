import { useEffect, useMemo, useRef, useState, type FormEvent } from 'react'
import { useQueryClient } from '@tanstack/react-query'
import { useNavigate } from 'react-router-dom'

import { useAuthenticatedSession } from '../auth/session'
import { ExternalAgentSettings } from '../external-agents/ExternalAgentSettings'
import { AppShell } from '../shell/AppShell'
import {
  ActorTag,
  Avatar,
  Banner,
  Button,
  ButtonLink,
  EmptyState,
  ErrorState,
  Icon,
  ListSkeleton,
  PageHeader,
  TextField,
  actorKind,
  describeError,
} from '../ui'
import { createGroup, directQueryKey, openDm, useDirectOverview } from './api'

/** Start a DM with one person or agent, or a group with several. */
export function MessagesPage() {
  const session = useAuthenticatedSession()
  const overview = useDirectOverview()
  const queryClient = useQueryClient()
  const navigate = useNavigate()
  const groupKey = useRef(crypto.randomUUID())
  const [selected, setSelected] = useState<string[]>([])
  const [filter, setFilter] = useState('')
  const [groupName, setGroupName] = useState('')
  const [busy, setBusy] = useState(false)
  const [error, setError] = useState<string | null>(null)
  const people = useMemo(() => overview.data?.people ?? [], [overview.data?.people])
  const needle = filter.trim().toLowerCase()
  const visible = people.filter(
    (person) => !needle || person.displayName.toLowerCase().includes(needle) || person.handle.toLowerCase().includes(needle),
  )
  const chosen = people.filter((person) => selected.includes(person.id))

  const toggle = (id: string) =>
    setSelected((current) => (current.includes(id) ? current.filter((candidate) => candidate !== id) : [...current, id]))

  const start = (event: FormEvent<HTMLFormElement>) => {
    event.preventDefault()
    if (!selected.length || busy) return
    setBusy(true)
    setError(null)
    const created =
      selected.length === 1
        ? openDm(selected[0])
        : createGroup({ name: groupName, memberIds: selected, idempotencyKey: groupKey.current })
    void created
      .then(async (conversationId) => {
        groupKey.current = crypto.randomUUID()
        await queryClient.invalidateQueries({ queryKey: directQueryKey })
        navigate(`/messages/${encodeURIComponent(conversationId)}`)
      })
      .catch((reason: unknown) => setError(`The conversation didn’t start. ${describeError(reason).message}`))
      .finally(() => setBusy(false))
  }

  const submitLabel =
    selected.length > 1
      ? `Start group with ${selected.length}`
      : chosen[0]
        ? `Message ${chosen[0].displayName}`
        : 'Start conversation'

  return (
    <AppShell direct mainClassName="aw-main--page" user={session.user}>
      <div className="aw-page aw-page--narrow">
        <PageHeader icon="compose" title="New message">
          Pick one person or agent for a direct message, or several for a group.
        </PageHeader>
        {overview.isPending ? <ListSkeleton label="Loading people" rows={2} avatars /> : null}
        {overview.isError ? (
          <ErrorState error={overview.error} onRetry={() => void overview.refetch()} title="Messages didn’t load" />
        ) : null}
        {overview.data && people.length === 0 ? (
          <EmptyState
            action={
              overview.data.capabilities.manageAgents ? (
                <ButtonLink icon="bot" to="/messages/agents" variant="primary">
                  Connect an agent
                </ButtonLink>
              ) : undefined
            }
            icon="users"
            title="No one to message yet"
          >
            {overview.data.capabilities.manageAgents
              ? 'Connect an agent to start. Each one gets its own identity and shows up here.'
              : 'Ask your workspace owner to connect an agent. Each one gets its own identity and shows up here.'}
          </EmptyState>
        ) : null}
        {people.length > 0 ? (
          <form className="aw-newmsg" onSubmit={start}>
            <div className="aw-newmsg__to">
              <span className="aw-newmsg__label">To</span>
              {chosen.map((person) => (
                <button
                  aria-label={`Remove ${person.displayName}`}
                  className="aw-newmsg__chip"
                  key={person.id}
                  onClick={() => toggle(person.id)}
                  type="button"
                >
                  <Avatar kind={actorKind(person.kind)} name={person.displayName} seed={person.id} size={20} />
                  {person.displayName}
                  <Icon name="x" size={14} />
                </button>
              ))}
              <input
                aria-label="Find people and agents"
                className="aw-newmsg__input"
                onChange={(event) => setFilter(event.target.value)}
                placeholder={chosen.length ? '' : 'Find people and agents'}
                value={filter}
              />
            </div>
            <fieldset className="aw-newmsg__people">
              <legend className="visually-hidden">People and agents</legend>
              {visible.length === 0 ? <p className="aw-newmsg__none">No one matches “{filter}”.</p> : null}
              {visible.map((person) => {
                const kind = actorKind(person.kind)
                const checked = selected.includes(person.id)
                return (
                  <label className={checked ? 'aw-person is-checked' : 'aw-person'} key={person.id}>
                    <input checked={checked} onChange={() => toggle(person.id)} type="checkbox" />
                    <Avatar kind={kind} name={person.displayName} seed={person.id} size={32} />
                    <span className="aw-person__name">{person.displayName}</span>
                    <ActorTag kind={kind} />
                    <span className="aw-person__handle">@{person.handle}</span>
                    <span aria-hidden="true" className="aw-person__check">
                      <Icon name="check" size={14} />
                    </span>
                  </label>
                )
              })}
            </fieldset>
            {selected.length > 1 ? (
              <TextField
                hint="Leave it blank to name the group after its members."
                id="group-name"
                label="Group name (optional)"
                maxLength={120}
                onChange={(event) => setGroupName(event.target.value)}
                value={groupName}
              />
            ) : null}
            {error ? <Banner tone="danger">{error}</Banner> : null}
            <div className="aw-form__actions">
              <Button busy={busy} disabled={!selected.length} type="submit" variant="primary">
                {submitLabel}
              </Button>
            </div>
          </form>
        ) : null}
      </div>
    </AppShell>
  )
}

/** Connect and manage the agents you can message. */
export function AgentsPage() {
  const session = useAuthenticatedSession()
  const overview = useDirectOverview()
  const queryClient = useQueryClient()
  // New agents should appear in the people list as soon as you leave here.
  useEffect(() => () => void queryClient.invalidateQueries({ queryKey: directQueryKey }), [queryClient])
  return (
    <AppShell direct mainClassName="aw-main--page" user={session.user}>
      <div className="aw-page">
        <PageHeader icon="bot" title="Agents">
          Each agent connects with its own credential and posts as itself. Any client that speaks MCP can connect.
        </PageHeader>
        {overview.data?.capabilities.manageAgents ? (
          <ExternalAgentSettings projectId={overview.data.projectId} />
        ) : overview.data ? (
          <Banner tone="info">Only the workspace owner can connect and manage agents.</Banner>
        ) : overview.isPending ? (
          <ListSkeleton label="Loading agents" rows={2} avatars />
        ) : null}
      </div>
    </AppShell>
  )
}
