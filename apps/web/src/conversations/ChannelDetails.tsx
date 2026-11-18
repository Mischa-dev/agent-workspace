import { useQuery } from '@tanstack/react-query'
import { Paperclip } from 'lucide-react'
import { Link } from 'react-router-dom'

import { callOp } from '../api/ops'
import { queryKeys } from '../api/queryKeys'
import { useProjectExperience } from '../experience/layout'
import { fileContentUrl, formatBytes } from '../files/api'
import { pinHref } from '../memory/pinHref'
import { usePins } from '../memory/hooks'
import { Button, EmptyState, ErrorState, Icon, IconButton, ListSkeleton, SegmentedControl, plainPreview } from '../ui'
import { workItemHref } from '../work-items/links'
import { ChannelMembers } from './ChannelMembers'
import { channelDetailsTabs, type ChannelDetailsTab } from './channelDetailsTabs'

/**
 * A channel's details beside it (C2.8): what it's for, who's in it, what's
 * pinned, the tasks it's about or made, and the files sent in it. Each tab
 * reads with the viewer's own access.
 */
export function ChannelDetails({
  canEditTopic,
  channelId,
  isPrivate,
  name,
  onClose,
  onEditTopic,
  onLeft,
  onTab,
  projectId,
  tab,
  topic,
  viewerId,
}: {
  canEditTopic: boolean
  channelId: string
  isPrivate: boolean
  name: string
  onClose: () => void
  onEditTopic: () => void
  onLeft: () => void
  onTab: (tab: ChannelDetailsTab) => void
  projectId: string
  tab: ChannelDetailsTab
  topic: string | null
  viewerId: string
}) {
  const experience = useProjectExperience(projectId)
  const tabs = channelDetailsTabs({ isPrivate, tasks: experience.shows('area.tasks'), files: experience.shows('area.files') })
  const shown = tabs.some((candidate) => candidate.value === tab) ? tab : 'about'
  return (
    <aside aria-label={`Details of #${name}`} className="context-drawer aw-chdetails">
      <div className="aw-details__head">
        <h2>#{name}</h2>
        <IconButton className="aw-panelclose" icon="x" label="Close details" onClick={onClose} size="sm" />
      </div>
      <SegmentedControl label="Details" onChange={onTab} options={tabs} value={shown} />
      <div className="aw-chdetails__body">
        {shown === 'about' ? (
          <section className="aw-chdetails__about">
            <h3>Topic</h3>
            <p>{topic || 'No topic yet.'}</p>
            {canEditTopic ? (
              <Button onClick={onEditTopic} size="sm">
                {topic ? 'Change topic' : 'Set a topic'}
              </Button>
            ) : null}
            <h3>Who can read it</h3>
            <p>{isPrivate ? 'Only its members.' : 'Everyone in this space.'}</p>
          </section>
        ) : shown === 'members' ? (
          <ChannelMembers channelId={channelId} embedded onClose={onClose} onLeft={onLeft} projectId={projectId} viewerId={viewerId} />
        ) : shown === 'pins' ? (
          <PinsTab channelId={channelId} projectId={projectId} />
        ) : shown === 'tasks' ? (
          <TasksTab channelId={channelId} projectId={projectId} />
        ) : (
          <FilesTab channelId={channelId} projectId={projectId} />
        )}
      </div>
    </aside>
  )
}

function PinsTab({ channelId, projectId }: { channelId: string; projectId: string }) {
  const pins = usePins(projectId, channelId)
  if (pins.isPending) return <ListSkeleton label="Loading pins" rows={3} />
  if (pins.isError) return <ErrorState error={pins.error} onRetry={() => void pins.refetch()} title="Pins didn’t load" />
  const items = pins.data.pins
  if (items.length === 0) {
    return (
      <EmptyState icon="bookmark" title="Nothing pinned">
        Pin a message, page or decision from its menu to keep it here.
      </EmptyState>
    )
  }
  return (
    <ul className="aw-chdetails__list">
      {items.map((item) => (
        <li key={`${item.kind}:${item.id}`}>
          {item.kind === 'message' ? (
            <a href={pinHref(projectId, item)}>{plainPreview(item.title, 120)}</a>
          ) : (
            <Link to={pinHref(projectId, item)}>{item.title}</Link>
          )}
          {item.text ? <span className="aw-chdetails__meta">{plainPreview(item.text, 120)}</span> : null}
        </li>
      ))}
    </ul>
  )
}

function TasksTab({ channelId, projectId }: { channelId: string; projectId: string }) {
  const experience = useProjectExperience(projectId)
  const tasks = useQuery({
    queryKey: queryKeys.conversationTasks(projectId, channelId),
    queryFn: () => callOp('conversation_task_list', { conversationId: channelId }, { retries: 0 }),
  })
  if (tasks.isPending) return <ListSkeleton label={`Loading ${experience.word('task', 'many')}`} rows={3} />
  if (tasks.isError) return <ErrorState error={tasks.error} onRetry={() => void tasks.refetch()} title={`${experience.label('task', 'many')} didn’t load`} />
  const items = tasks.data.tasks
  if (items.length === 0) {
    return (
      <EmptyState icon="checklist" title={`No ${experience.word('task', 'many')} yet`}>
        Make one from a message here, or link one to this channel.
      </EmptyState>
    )
  }
  return (
    <ul className="aw-chdetails__list">
      {items.map((task) => (
        <li key={task.id}>
          <Link to={workItemHref(projectId, task.id)}>
            {experience.shows('tech.task_keys') ? <span className="aw-chdetails__key">{task.key}</span> : null}
            {task.title}
          </Link>
          <span className="aw-chdetails__meta">
            {task.how === 'made_from' ? 'Made from a message here' : 'Linked here'} · {task.statusCategory.replace(/_/g, ' ')}
          </span>
        </li>
      ))}
    </ul>
  )
}

function FilesTab({ channelId, projectId }: { channelId: string; projectId: string }) {
  const files = useQuery({
    queryKey: queryKeys.conversationFiles(projectId, channelId),
    queryFn: () => callOp('conversation_file_list', { conversationId: channelId }, { retries: 0 }),
  })
  if (files.isPending) return <ListSkeleton label="Loading files" rows={3} />
  if (files.isError) return <ErrorState error={files.error} onRetry={() => void files.refetch()} title="Files didn’t load" />
  const items = files.data.files
  if (items.length === 0) {
    return (
      <EmptyState icon="link" title="No files yet">
        Files sent in this channel show here.
      </EmptyState>
    )
  }
  return (
    <ul className="aw-chdetails__list">
      {items.map((file) => (
        <li key={file.id}>
          <a href={fileContentUrl(file.id)} rel="noreferrer" target="_blank">
            <Icon name={Paperclip} size={14} /> {file.name}
          </a>
          <span className="aw-chdetails__meta">
            {formatBytes(file.size)} · <a href={`#message-${file.messageId}`}>In the conversation</a>
          </span>
        </li>
      ))}
    </ul>
  )
}
