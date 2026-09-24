import { FormEvent, useEffect, useRef, useState } from 'react'
import { useQueryClient } from '@tanstack/react-query'
import { Link, useNavigate } from 'react-router-dom'

import { useAuthenticatedSession } from '../auth/session'
import { ExternalAgentSettings } from '../external-agents/ExternalAgentSettings'
import { AppShell } from '../shell/AppShell'
import { createGroup, directQueryKey, openDm, useDirectOverview } from './api'

/** Start a DM with one person or a group with several. */
export function MessagesPage() {
  const session = useAuthenticatedSession()
  const overview = useDirectOverview()
  const queryClient = useQueryClient()
  const navigate = useNavigate()
  const groupKey = useRef(crypto.randomUUID())
  const [selected, setSelected] = useState<string[]>([])
  const [groupName, setGroupName] = useState('')
  const [busy, setBusy] = useState(false)
  const [error, setError] = useState<string | null>(null)
  const people = overview.data?.people ?? []

  const toggle = (id: string) =>
    setSelected((current) =>
      current.includes(id)
        ? current.filter((candidate) => candidate !== id)
        : [...current, id],
    )

  const start = (event: FormEvent<HTMLFormElement>) => {
    event.preventDefault()
    if (!selected.length || busy) return
    setBusy(true)
    setError(null)
    const created =
      selected.length === 1
        ? openDm(selected[0])
        : createGroup({
            name: groupName,
            memberIds: selected,
            idempotencyKey: groupKey.current,
          })
    void created
      .then(async (conversationId) => {
        groupKey.current = crypto.randomUUID()
        await queryClient.invalidateQueries({ queryKey: directQueryKey })
        navigate(`/messages/${encodeURIComponent(conversationId)}`)
      })
      .catch((reason: unknown) =>
        setError(
          reason instanceof Error ? reason.message : 'Could not start the conversation.',
        ),
      )
      .finally(() => setBusy(false))
  }

  return (
    <AppShell direct mainClassName="workspace" user={session.user}>
      <header className="workspace-heading">
        <div>
          <span className="section-label">Messages</span>
          <h1 id="workspace-heading">New message</h1>
        </div>
      </header>
      {overview.isPending ? <p role="status">Loading…</p> : null}
      {overview.isError ? (
        <div className="error-state" role="alert">
          <h2>Messages are unavailable</h2>
          <button onClick={() => void overview.refetch()} type="button">
            Try again
          </button>
        </div>
      ) : null}
      {overview.data && people.length === 0 ? (
        <div className="empty-state">
          <h2>No one to message yet.</h2>
          <p>
            Add your agents first. Each one gets its own identity and
            connection.
          </p>
          {overview.data.capabilities.manageAgents ? (
            <Link to="/messages/agents">Add an agent</Link>
          ) : null}
        </div>
      ) : null}
      {people.length > 0 ? (
        <form className="new-message-form" onSubmit={start}>
          <fieldset>
            <legend>
              Pick one person for a DM, or several for a group.
            </legend>
            <ul className="people-picker">
              {people.map((person) => (
                <li key={person.id}>
                  <label>
                    <input
                      checked={selected.includes(person.id)}
                      onChange={() => toggle(person.id)}
                      type="checkbox"
                    />
                    <span>{person.displayName}</span>
                    <small>
                      {person.kind === 'external_agent' ? 'agent' : `@${person.handle}`}
                    </small>
                  </label>
                </li>
              ))}
            </ul>
          </fieldset>
          {selected.length > 1 ? (
            <div className="field">
              <label htmlFor="group-name">Group name (optional)</label>
              <input
                id="group-name"
                maxLength={120}
                onChange={(event) => setGroupName(event.target.value)}
                placeholder="Named after its members if left blank"
                value={groupName}
              />
            </div>
          ) : null}
          <button disabled={!selected.length || busy} type="submit">
            {busy
              ? 'Starting…'
              : selected.length > 1
                ? `Start group with ${selected.length}`
                : 'Open DM'}
          </button>
          {error ? <p role="alert">{error}</p> : null}
        </form>
      ) : null}
    </AppShell>
  )
}

/** Add and manage the agents you can DM. */
export function AgentsPage() {
  const session = useAuthenticatedSession()
  const overview = useDirectOverview()
  const queryClient = useQueryClient()
  // New agents should appear in the people list as soon as you leave here.
  useEffect(
    () => () => void queryClient.invalidateQueries({ queryKey: directQueryKey }),
    [queryClient],
  )
  return (
    <AppShell direct mainClassName="workspace" user={session.user}>
      <header className="workspace-heading">
        <div>
          <span className="section-label">Messages</span>
          <h1 id="workspace-heading">Agents</h1>
        </div>
      </header>
      <p>
        Each agent connects with its own credential and posts as itself. Any
        client that speaks MCP can connect.
      </p>
      {overview.data?.capabilities.manageAgents ? (
        <ExternalAgentSettings projectId={overview.data.projectId} />
      ) : overview.data ? (
        <p>Only the workspace owner can manage agents.</p>
      ) : null}
    </AppShell>
  )
}
