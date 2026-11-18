import { useState } from 'react'
import { useQuery, useQueryClient } from '@tanstack/react-query'
import { Link, useParams } from 'react-router-dom'

import { callOp } from '../api/ops'
import { queryKeys } from '../api/queryKeys'
import { toastError } from '../app/mutations'
import { ProjectPageError, ProjectPageLoading } from '../projects/ProjectPageStates'
import { useProject } from '../projects/hooks'
import { useShellPage } from '../shell/shellStore'
import { Button, EmptyState, ErrorState, Icon, ListSkeleton, PageHeader, TextField, showToast } from '../ui'
import './chat.css'
import '../styles/pages.css'

/** Every channel of a space you can see, joined or not, to find and join (C2.7). */
export function ChannelBrowserPage() {
  const { projectId = '' } = useParams()
  const project = useProject(projectId)
  const queryClient = useQueryClient()
  const [query, setQuery] = useState('')
  const [busy, setBusy] = useState<string | null>(null)
  const channels = useQuery({
    queryKey: queryKeys.channelList(projectId),
    queryFn: () => callOp('channel_list', { spaceId: projectId }, { retries: 0 }),
    enabled: Boolean(projectId),
  })

  useShellPage(project.data ? { mainClassName: 'aw-main--page', project: project.data } : {})
  if (project.isPending) return <ProjectPageLoading />
  if (project.isError) return <ProjectPageError error={project.error} onRetry={() => void project.refetch()} />

  const words = query.trim().toLowerCase()
  const shown = (channels.data?.channels ?? []).filter(
    (channel) => !words || channel.name.toLowerCase().includes(words) || (channel.topic ?? '').toLowerCase().includes(words),
  )
  const base = `/projects/${encodeURIComponent(projectId)}`
  const refresh = () =>
    Promise.all([
      queryClient.invalidateQueries({ queryKey: queryKeys.channelList(projectId) }),
      queryClient.invalidateQueries({ queryKey: queryKeys.conversationSidebarRoot(projectId) }),
    ])
  const change = (channelId: string, name: string, join: boolean) => {
    setBusy(channelId)
    void callOp(join ? 'channel_join' : 'channel_leave', { channelId })
      .then(async () => {
        await refresh()
        showToast({ tone: 'success', title: join ? `Joined #${name}` : `Left #${name}` })
      })
      .catch((reason: unknown) => toastError(join ? `You didn’t join #${name}` : `You didn’t leave #${name}`, reason))
      .finally(() => setBusy(null))
  }

  return (
    <div className="aw-page aw-page--reading">
      <PageHeader icon="hash" title="Browse channels" />
      <TextField
        autoComplete="off"
        label="Find a channel"
        onChange={(event) => setQuery(event.target.value)}
        placeholder="Search by name or topic"
        type="search"
        value={query}
      />
      {channels.isPending ? <ListSkeleton label="Loading channels" rows={4} /> : null}
      {channels.isError ? <ErrorState error={channels.error} onRetry={() => void channels.refetch()} title="Channels didn’t load" /> : null}
      {channels.isSuccess && shown.length === 0 ? (
        <EmptyState icon="hash" title={words ? 'No channel matches' : 'No channels yet'}>
          {words ? 'Try other words, or start a channel from the sidebar.' : 'Start one from the sidebar.'}
        </EmptyState>
      ) : null}
      {shown.length ? (
        <ul className="aw-archive">
          {shown.map((channel) => (
            <li className="aw-archive__item" key={channel.id}>
              <Link className="aw-archive__row" to={`${base}/channels/${encodeURIComponent(channel.id)}`}>
                <Icon name={channel.private ? 'lock' : 'hash'} />
                <span className="aw-archive__name">{channel.name}</span>
                <span className="aw-archive__meta">
                  {channel.memberCount === 1 ? '1 member' : `${channel.memberCount} members`}
                  {channel.topic ? ` · ${channel.topic}` : ''}
                </span>
                <Icon className="aw-archive__chev" name="chevron-right" />
              </Link>
              {channel.joined ? (
                <Button busy={busy === channel.id} disabled={busy !== null} onClick={() => change(channel.id, channel.name, false)} size="sm" variant="ghost">
                  Leave
                </Button>
              ) : (
                <Button busy={busy === channel.id} disabled={busy !== null} onClick={() => change(channel.id, channel.name, true)} size="sm">
                  Join
                </Button>
              )}
            </li>
          ))}
        </ul>
      ) : null}
    </div>
  )
}
