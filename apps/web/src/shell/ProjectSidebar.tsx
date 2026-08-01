import { FormEvent, useRef, useState } from 'react'
import { useQueryClient } from '@tanstack/react-query'
import { NavLink, useLocation, useNavigate } from 'react-router-dom'

import { queryKeys } from '../api/queryKeys'
import { createChannel } from '../conversations/api'
import { useProjectConversationSidebar } from '../conversations/hooks'
import type { SidebarThread } from '../conversations/types'
import type { Project } from '../projects/types'

const threadAttentionIcons: Record<SidebarThread['attention'], string> = {
  response_required: '!',
  active: '↻',
  failed: '×',
  completed: '✓',
  unread: '●',
  settled: '·',
}

export function ProjectSidebar({
  mobileOpen,
  onNavigate,
  project,
  selectedChannelId = '',
  thread,
}: {
  mobileOpen: boolean
  onNavigate: () => void
  project: Project
  selectedChannelId?: string
  thread?: { id: string; name: string } | null
}) {
  const location = useLocation()
  const expandSelected = new URLSearchParams(location.search).get('threads') === 'all'
  const sidebar = useProjectConversationSidebar(project.id, selectedChannelId, {
    expandedChannelId:
      expandSelected && selectedChannelId ? selectedChannelId : undefined,
  })
  return (
    <aside
      aria-label={`${project.name} navigation`}
      className="project-sidebar"
      data-mobile-open={mobileOpen}
      id="project-sidebar"
    >
      <header className="project-heading">
        <span className="section-label">{project.key}</span>
        <p>{project.name}</p>
      </header>
      <nav aria-label="Project">
        <ul>
          <li>
            <NavLink
              end
              onClick={onNavigate}
              to={`/projects/${encodeURIComponent(project.id)}`}
            >
              <span aria-hidden="true">⌂</span>
              Overview
            </NavLink>
          </li>
          <li>
            <NavLink
              onClick={onNavigate}
              to={`/projects/${encodeURIComponent(project.id)}/work-items`}
            >
              <span aria-hidden="true">◇</span>
              Work Items
            </NavLink>
          </li>
          {sidebar.data?.channels.map((channel) => (
            <li className="channel-navigation" key={channel.id}>
              <NavLink
                onClick={onNavigate}
                to={`/projects/${encodeURIComponent(project.id)}/channels/${encodeURIComponent(channel.id)}`}
              >
                <span aria-hidden="true">#</span>
                <span>{channel.name}</span>
                {channel.unread ? (
                  <span aria-label="Unread Messages" className="unread-dot">
                    ●
                  </span>
                ) : null}
              </NavLink>
              {channel.threads.length > 0 ? (
                <ul aria-label={`${channel.name} Threads`}>
                  {channel.threads.map((channelThread) => (
                    <li key={channelThread.id}>
                      <NavLink
                        className="thread-link"
                        data-attention={channelThread.attention}
                        onClick={onNavigate}
                        to={`/projects/${encodeURIComponent(project.id)}/threads/${encodeURIComponent(channelThread.id)}`}
                      >
                        <span aria-hidden="true">↳</span>
                        <span>{channelThread.name}</span>
                        <span
                          className="thread-attention"
                          data-attention={channelThread.attention}
                        >
                          <span
                            aria-hidden="true"
                            className="thread-attention-icon"
                          >
                            {threadAttentionIcons[channelThread.attention]}
                          </span>
                          <span className="thread-attention-label">
                            {channelThread.attentionLabel}
                          </span>
                        </span>
                      </NavLink>
                    </li>
                  ))}
                </ul>
              ) : null}
              {channel.hasMoreThreads ? (
                <>
                  <NavLink
                    className="show-more-link"
                    onClick={onNavigate}
                    to={`/projects/${encodeURIComponent(project.id)}/channels/${encodeURIComponent(channel.id)}?threads=all`}
                  >
                    Show more Threads
                  </NavLink>
                  <NavLink
                    className="show-more-link"
                    onClick={onNavigate}
                    to={`/projects/${encodeURIComponent(project.id)}/channels/${encodeURIComponent(channel.id)}?threads=all`}
                  >
                    View all Threads
                  </NavLink>
                </>
              ) : null}
            </li>
          ))}
          {thread &&
            !sidebar.data?.channels.some((channel) =>
              channel.threads.some((candidate) => candidate.id === thread.id),
            ) && (
            <li>
              <NavLink
                className="thread-link"
                onClick={onNavigate}
                to={`/projects/${encodeURIComponent(project.id)}/threads/${encodeURIComponent(thread.id)}`}
              >
                <span aria-hidden="true">↳</span>
                {thread.name}
              </NavLink>
            </li>
          )}
          {sidebar.data?.capabilities.createChannel ? (
            <li>
              <ChannelCreator onNavigate={onNavigate} projectId={project.id} />
            </li>
          ) : null}
          <li>
            <NavLink
              onClick={onNavigate}
              to={`/projects/${encodeURIComponent(project.id)}/conversations/archived`}
            >
              <span aria-hidden="true">□</span>
              Archived Conversations
            </NavLink>
          </li>
          <li>
            <NavLink
              onClick={onNavigate}
              to={`/projects/${encodeURIComponent(project.id)}/settings`}
            >
              <span aria-hidden="true">⚙</span>
              Settings
            </NavLink>
          </li>
        </ul>
      </nav>
    </aside>
  )
}

function ChannelCreator({
  onNavigate,
  projectId,
}: {
  onNavigate: () => void
  projectId: string
}) {
  const queryClient = useQueryClient()
  const navigate = useNavigate()
  const idempotencyKey = useRef(crypto.randomUUID())
  const [name, setName] = useState('')
  const [busy, setBusy] = useState(false)
  const [error, setError] = useState<string | null>(null)

  const submit = (event: FormEvent<HTMLFormElement>) => {
    event.preventDefault()
    const value = name.trim()
    if (!value || busy) return
    setBusy(true)
    setError(null)
    void createChannel({
      projectId,
      name: value,
      idempotencyKey: idempotencyKey.current,
    })
      .then((channel) => {
        idempotencyKey.current = crypto.randomUUID()
        setName('')
        void queryClient.invalidateQueries({
          queryKey: queryKeys.conversationSidebarRoot(projectId),
        })
        onNavigate()
        navigate(
          `/projects/${encodeURIComponent(projectId)}/channels/${encodeURIComponent(channel.id)}`,
        )
      })
      .catch((reason: unknown) => {
        setError(reason instanceof Error ? reason.message : 'Channel could not be created.')
      })
      .finally(() => setBusy(false))
  }

  return (
    <details className="channel-creator">
      <summary>New Channel</summary>
      <form onSubmit={submit}>
        <label className="visually-hidden" htmlFor={`channel-name-${projectId}`}>
          Channel name
        </label>
        <input
          id={`channel-name-${projectId}`}
          maxLength={120}
          onChange={(event) => setName(event.target.value)}
          placeholder="Channel name"
          value={name}
        />
        <button disabled={!name.trim() || busy} type="submit">
          {busy ? 'Creating…' : 'Create'}
        </button>
        {error ? <p role="alert">{error}</p> : null}
      </form>
    </details>
  )
}
