import { useRef, useState, type FormEvent } from 'react'
import { useQueryClient } from '@tanstack/react-query'
import { useLocation, useNavigate } from 'react-router-dom'

import { queryKeys } from '../api/queryKeys'
import { createChannel } from '../conversations/api'
import { useProjectConversationSidebar } from '../conversations/hooks'
import type { SidebarThread } from '../conversations/types'
import type { Project } from '../projects/types'
import { Button, Dialog, TextField, type MenuEntry } from '../ui'
import { SidebarFrame, SidebarLink, SidebarSection, type SidebarAttention } from './SidebarParts'

const attentionMarks: Partial<Record<SidebarThread['attention'], SidebarAttention>> = {
  response_required: 'needs-you',
  active: 'working',
  failed: 'failed',
  completed: 'done',
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
  const navigate = useNavigate()
  const [creating, setCreating] = useState(false)
  const expandSelected = new URLSearchParams(location.search).get('threads') === 'all'
  const sidebar = useProjectConversationSidebar(project.id, selectedChannelId, {
    expandedChannelId: expandSelected && selectedChannelId ? selectedChannelId : undefined,
  })
  const base = `/projects/${encodeURIComponent(project.id)}`
  const canCreate = Boolean(sidebar.data?.capabilities.createChannel)
  const go = (to: string) => {
    onNavigate()
    navigate(to)
  }
  const menu: MenuEntry[] = [
    ...(canCreate ? [{ label: 'New channel', icon: 'plus' as const, onSelect: () => setCreating(true) }] : []),
    { label: 'Archived conversations', icon: 'archive', onSelect: () => go(`${base}/conversations/archived`) },
    { label: 'Project settings', icon: 'settings', onSelect: () => go(`${base}/settings`) },
  ]
  const listedThread = sidebar.data?.channels.some((channel) => channel.threads.some((candidate) => candidate.id === thread?.id))

  return (
    <SidebarFrame id="project-sidebar" label={`${project.name} navigation`} menu={menu} mobileOpen={mobileOpen} title={project.name}>
      <ul className="aw-section__list aw-sidebar__top">
        <SidebarLink end icon="home" label="Overview" onNavigate={onNavigate} to={base} />
        <SidebarLink icon="work" label="Work items" onNavigate={onNavigate} to={`${base}/work-items`} />
        <SidebarLink icon="checklist" label="Milestones" onNavigate={onNavigate} to={`${base}/milestones`} />
        <SidebarLink icon="clock" label="Launches" onNavigate={onNavigate} to={`${base}/launches`} />
        <SidebarLink icon="list" label="Pages" onNavigate={onNavigate} to={`${base}/pages`} />
        <SidebarLink icon="check" label="Decisions" onNavigate={onNavigate} to={`${base}/decisions`} />
        <SidebarLink icon="search" label="Search" onNavigate={onNavigate} to={`/search?projectId=${encodeURIComponent(project.id)}`} />
      </ul>
      <SidebarSection addLabel="New channel" label="Channels" onAdd={canCreate ? () => setCreating(true) : undefined}>
        {(sidebar.data?.channels ?? []).flatMap((channel) => {
          const selected = channel.id === selectedChannelId
          const rows = [
            <SidebarLink
              icon="hash"
              keep={selected || channel.unread}
              key={channel.id}
              label={channel.name}
              onNavigate={onNavigate}
              to={`${base}/channels/${encodeURIComponent(channel.id)}`}
              unread={channel.unread}
            />,
            ...channel.threads.map((channelThread) => (
              <SidebarLink
                attention={attentionMarks[channelThread.attention]}
                attentionLabel={channelThread.attentionLabel}
                dataAttention={channelThread.attention}
                keep={channelThread.attention === 'response_required' || channelThread.id === thread?.id}
                key={channelThread.id}
                label={channelThread.name}
                nested
                onNavigate={onNavigate}
                to={`${base}/threads/${encodeURIComponent(channelThread.id)}`}
                unread={channelThread.attention === 'unread'}
              />
            )),
          ]
          if (channel.hasMoreThreads) {
            rows.push(
              <SidebarLink
                key={`${channel.id}-more`}
                label="All threads"
                nested
                onNavigate={onNavigate}
                to={`${base}/channels/${encodeURIComponent(channel.id)}?threads=all`}
              />,
            )
          }
          return rows
        })}
        {thread && sidebar.data && !listedThread ? (
          <SidebarLink
            keep
            key={thread.id}
            label={thread.name}
            nested
            onNavigate={onNavigate}
            to={`${base}/threads/${encodeURIComponent(thread.id)}`}
          />
        ) : null}
      </SidebarSection>
      {sidebar.isError ? <p className="aw-sidebar__note">Channels couldn’t load. They’ll retry on their own.</p> : null}
      <ChannelCreator
        onClose={() => setCreating(false)}
        onCreated={(to) => go(to)}
        open={creating}
        projectId={project.id}
      />
    </SidebarFrame>
  )
}

function ChannelCreator({
  onClose,
  onCreated,
  open,
  projectId,
}: {
  onClose: () => void
  onCreated: (to: string) => void
  open: boolean
  projectId: string
}) {
  const queryClient = useQueryClient()
  const idempotencyKey = useRef(crypto.randomUUID())
  const [name, setName] = useState('')
  const [busy, setBusy] = useState(false)
  const [error, setError] = useState<string | null>(null)
  const value = name.trim().replace(/^#/, '')

  const submit = (event: FormEvent<HTMLFormElement>) => {
    event.preventDefault()
    if (!value || busy) return
    setBusy(true)
    setError(null)
    void createChannel({ projectId, name: value, idempotencyKey: idempotencyKey.current })
      .then((channel) => {
        idempotencyKey.current = crypto.randomUUID()
        setName('')
        onClose()
        void queryClient.invalidateQueries({ queryKey: queryKeys.conversationSidebarRoot(projectId) })
        onCreated(`/projects/${encodeURIComponent(projectId)}/channels/${encodeURIComponent(channel.id)}`)
      })
      .catch((reason: unknown) => setError(reason instanceof Error ? reason.message : 'The channel wasn’t created. Try again.'))
      .finally(() => setBusy(false))
  }

  return (
    <Dialog
      description="Channels are where a Project’s conversations happen. Everyone in the Project can read and post."
      onClose={onClose}
      open={open}
      title="New channel"
    >
      <form className="aw-form" id="new-channel-form" onSubmit={submit}>
        <TextField
          autoComplete="off"
          error={error ?? undefined}
          hint="Short and lowercase works best, like design or deploys."
          label="Name"
          maxLength={120}
          onChange={(event) => setName(event.target.value)}
          prefix="#"
          value={name}
        />
        <div className="aw-form__actions">
          <Button onClick={onClose}>Cancel</Button>
          <Button busy={busy} disabled={!value} type="submit" variant="primary">
            Create channel
          </Button>
        </div>
      </form>
    </Dialog>
  )
}
