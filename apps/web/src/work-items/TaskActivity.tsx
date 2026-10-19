import { useQueryClient } from '@tanstack/react-query'
import { Eye } from 'lucide-react'
import { useState, type ReactNode } from 'react'

import { searchMentionableActors } from '../conversations/api'
import { MessageContent } from '../conversations/MessageContent'
import { StructuredComposer } from '../conversations/StructuredComposer'
import { Attachments } from '../files/Attachments'
import { uploadFile, type FileAttachment } from '../files/api'
import { ActorTag, Avatar, AvatarStack, Banner, Button, Icon, MessageSkeleton, actorKind, formatFullDate, formatRelative } from '../ui'
import { searchWorkItemReferences } from './api'
import { formatChanges, type FormattedChange } from './historyFormat'
import { taskFilesKey, useCreateWorkItemComment, useTaskFiles, useWorkItemTimeline } from './hooks'
import { useSetWatching } from './planningHooks'
import type { WorkItem, WorkItemTimelineEntry } from './types'

/** "Priority High → Urgent", with the old value struck through for sighted readers. */
function ChangeLine({ change }: { change: Extract<FormattedChange, { kind: 'field' }> }) {
  return (
    <span className="aw-change">
      <span className="aw-change__field">{change.label}</span>{' '}
      <del className="aw-change__from">{change.from}</del>
      <span aria-label="changed to" className="aw-change__arrow" role="img">
        {' → '}
      </span>
      <ins className="aw-change__to">{change.to}</ins>
    </span>
  )
}

function DescriptionDiff({ change }: { change: Extract<FormattedChange, { kind: 'description' }> }) {
  const [open, setOpen] = useState(false)
  const added = change.lines.filter((line) => line.kind === 'added').length
  const removed = change.lines.filter((line) => line.kind === 'removed').length
  return (
    <div className="aw-diff">
      <button aria-expanded={open} className="aw-diff__toggle" onClick={() => setOpen(!open)} type="button">
        <Icon className={open ? 'aw-tasks__chevron is-open' : 'aw-tasks__chevron'} name="chevron-right" size={12} />
        Description changed
        <span className="aw-diff__stats">
          +{added} −{removed}
        </span>
      </button>
      {open ? (
        <pre className="aw-diff__body">
          {change.lines.map((line, index) => (
            <span className={`aw-diff__line is-${line.kind}`} key={index}>
              <span aria-hidden="true" className="aw-diff__sign">
                {line.kind === 'added' ? '+' : line.kind === 'removed' ? '−' : ' '}
              </span>
              {line.kind === 'added' ? <span className="visually-hidden">Added: </span> : line.kind === 'removed' ? <span className="visually-hidden">Removed: </span> : null}
              {line.text || ' '}
              {'\n'}
            </span>
          ))}
          {change.truncated ? <span className="aw-diff__more">Longer changes are cut short here.</span> : null}
        </pre>
      ) : null}
    </div>
  )
}

/** History written before tasks were called tasks still says "Work Item". */
function eventSummary(summary: string) {
  return summary.replace(/\b[Ww]ork [Ii]tem(s?)\b/g, (_match, plural: string, offset: number) => `${offset === 0 ? 'Task' : 'task'}${plural}`)
}

// Runs of changes longer than this fold into "Show N more changes" between their first and last entries.
const FOLD_AFTER = 4

type Group = { kind: 'comment'; entry: WorkItemTimelineEntry } | { kind: 'events'; entries: WorkItemTimelineEntry[] }

function groupTimeline(entries: WorkItemTimelineEntry[]): Group[] {
  const groups: Group[] = []
  for (const entry of entries) {
    const last = groups[groups.length - 1]
    if (entry.comment) groups.push({ kind: 'comment', entry })
    else if (last?.kind === 'events') last.entries.push(entry)
    else groups.push({ kind: 'events', entries: [entry] })
  }
  return groups
}

/** Comments and changes, oldest first, with the comment box at the end; long runs of changes fold away. */
export function TaskActivity({ disabled, item, projectArchived }: { disabled: boolean; item: WorkItem; projectArchived: boolean }) {
  const timeline = useWorkItemTimeline(item.projectId, item.id)
  const createComment = useCreateWorkItemComment()
  const queryClient = useQueryClient()
  const files = useTaskFiles(item.projectId, item.id)
  const [unfolded, setUnfolded] = useState<ReadonlySet<number>>(() => new Set())
  const commentFiles = new Map<string, FileAttachment[]>()
  for (const file of files.data ?? []) {
    if (file.commentId) commentFiles.set(file.commentId, [...(commentFiles.get(file.commentId) ?? []), file])
  }

  const event = (entry: WorkItemTimelineEntry) => {
    const actor = entry.activity?.actor
    const changes = formatChanges(entry.activity?.changes, item.estimateUnit)
    return (
      <li className="aw-activity__event" key={entry.sequence}>
        <span aria-hidden="true" className="aw-activity__dot" />
        <div className="aw-activity__eventbody">
          <p className="aw-activity__line">
            <strong>{actor?.displayName ?? 'Someone'}</strong>
            {changes.length === 1 && changes[0].kind === 'field' ? <ChangeLine change={changes[0]} /> : <span>{eventSummary(entry.activity?.summary ?? '')}</span>}
            {entry.activity?.via ? <span className="aw-activity__via">via {entry.activity.via}</span> : null}
            <time className="aw-activity__time" dateTime={entry.createdAt} title={formatFullDate(entry.createdAt)}>
              {formatRelative(entry.createdAt)}
            </time>
          </p>
          {changes.length > 1 || changes[0]?.kind === 'description' ? (
            <ul className="aw-changes">
              {changes.map((change) => (
                <li key={change.field}>{change.kind === 'field' ? <ChangeLine change={change} /> : <DescriptionDiff change={change} />}</li>
              ))}
            </ul>
          ) : null}
        </div>
      </li>
    )
  }

  const comment = (entry: WorkItemTimelineEntry) => {
    const posted = entry.comment
    if (!posted) return null
    const kind = actorKind(posted.author.kind ?? 'human')
    return (
      <li className="aw-activity__comment" key={entry.sequence}>
        <Avatar kind={kind} name={posted.author.displayName} seed={posted.author.id} size={28} />
        <div className="aw-activity__body">
          <p className="aw-activity__line">
            <strong>{posted.author.displayName}</strong>
            <ActorTag kind={kind} />
            {posted.via ? <span className="aw-activity__via">via {posted.via}</span> : null}
            <time className="aw-activity__time" dateTime={entry.createdAt} title={formatFullDate(entry.createdAt)}>
              {formatRelative(entry.createdAt)}
            </time>
          </p>
          <div className="aw-activity__content">
            <MessageContent
              actors={posted.mentionedActors.map((mentioned) => ({
                ...mentioned,
                kind: mentioned.kind as 'human' | 'external_agent' | 'execution_worker' | 'integration' | 'system',
                avatarUrl: null,
              }))}
              content={posted.content}
              fallbackPlainText={posted.plainText}
              workItems={posted.referencedWorkItems}
            />
            <Attachments files={commentFiles.get(posted.id) ?? []} />
          </div>
        </div>
      </li>
    )
  }

  const rows: ReactNode[] = []
  groupTimeline([...(timeline.data ?? [])].reverse()).forEach((group) => {
    if (group.kind === 'comment') {
      rows.push(comment(group.entry))
      return
    }
    const { entries } = group
    const first = entries[0].sequence
    if (entries.length <= FOLD_AFTER || unfolded.has(first)) {
      rows.push(...entries.map(event))
      return
    }
    const hidden = entries.length - 3
    rows.push(event(entries[0]))
    rows.push(
      <li className="aw-activity__fold" key={`fold-${first}`}>
        <Button icon="chevron-down" onClick={() => setUnfolded(new Set([...unfolded, first]))} size="sm" variant="ghost">
          Show {hidden} more changes
        </Button>
      </li>,
    )
    rows.push(...entries.slice(-2).map(event))
  })

  return (
    <section aria-labelledby={`activity-${item.id}`} className="aw-tsection aw-activitysection">
      <header className="aw-tsection__head">
        <h2 className="aw-tsection__title" id={`activity-${item.id}`}>
          Activity
        </h2>
        <WatchControl item={item} readOnly={projectArchived} />
      </header>
      {timeline.isPending ? <MessageSkeleton label="Loading activity" rows={2} /> : null}
      {timeline.isError ? <Banner tone="danger">Activity didn’t load.</Banner> : null}
      {rows.length ? <ol className="aw-activity">{rows}</ol> : null}
      {!disabled ? (
        <StructuredComposer
          disabled={createComment.isPending}
          draftKey={`comment:${item.id}`}
          label="Add a comment"
          onSubmit={async ({ attachments, content, idempotencyKey, text }) => {
            // A comment needs words; one that is only files says what it attaches.
            const words =
              text || !attachments?.length
                ? content
                : { version: 1 as const, nodes: [{ type: 'text' as const, text: `Attached ${attachments.map((file) => file.name).join(', ')}`.slice(0, 2000) }] }
            await createComment.mutateAsync({ projectId: item.projectId, workItemId: item.id, content: words, idempotencyKey, attachments })
            if (attachments?.length) void queryClient.invalidateQueries({ queryKey: taskFilesKey(item.projectId, item.id) })
          }}
          placeholder="Leave a comment. @mention someone or reference a task."
          searchActors={async (query) => (await searchMentionableActors(item.projectId, query)).actors}
          searchWorkItems={(query) => searchWorkItemReferences(item.projectId, query)}
          submitLabel="Comment"
          upload={uploadFile({ kind: 'task', projectId: item.projectId, workItemId: item.id, forComment: true })}
        />
      ) : null}
    </section>
  )
}

/** Who gets told about changes, and a toggle for you. Anyone who can see a task can watch it, unless its project is archived. */
function WatchControl({ item, readOnly }: { item: WorkItem; readOnly: boolean }) {
  const watch = useSetWatching()
  const names = item.watchers.map((watcher) => watcher.displayName).join(', ')
  return (
    <div className="aw-watch">
      {item.watchers.length ? (
        <span className="aw-watch__people" title={`Watching: ${names}`}>
          <AvatarStack people={item.watchers.map((watcher) => ({ id: watcher.id, name: watcher.displayName, kind: actorKind(watcher.kind) }))} size={20} />
          <span className="visually-hidden">Watched by {names}</span>
          {item.watchers.length > 3 ? <span className="aw-watch__more">+{item.watchers.length - 3}</span> : null}
        </span>
      ) : null}
      {readOnly ? null : (
        <Button
          aria-pressed={item.watching}
          busy={watch.isPending}
          className="aw-watch__toggle"
          icon={Eye}
          onClick={() => watch.mutate({ projectId: item.projectId, workItemId: item.id, watching: !item.watching, idempotencyKey: crypto.randomUUID() })}
          size="sm"
          variant="ghost"
        >
          {item.watching ? 'Watching' : 'Watch'}
        </Button>
      )}
      {watch.isError ? (
        <span className="aw-prop__error" role="alert">
          That didn’t save. Try again.
        </span>
      ) : null}
    </div>
  )
}
