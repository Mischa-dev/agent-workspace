import { useQueryClient } from '@tanstack/react-query'
import { useState } from 'react'

import { searchMentionableActors } from '../conversations/api'
import { MessageContent } from '../conversations/MessageContent'
import { StructuredComposer } from '../conversations/StructuredComposer'
import { queryKeys } from '../api/queryKeys'
import { Attachments } from '../files/Attachments'
import { uploadFile, type FileAttachment } from '../files/api'
import { ActorTag, Avatar, Banner, Icon, MessageSkeleton, actorKind, formatDateTime, formatFullDate } from '../ui'
import { searchWorkItemReferences } from './api'
import { formatChanges, type FormattedChange } from './historyFormat'
import { useCreateWorkItemComment, useTaskFiles, useWorkItemTimeline } from './hooks'
import type { WorkItem } from './types'

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

export function TaskActivity({ disabled, item }: { disabled: boolean; item: WorkItem }) {
  const timeline = useWorkItemTimeline(item.projectId, item.id)
  const createComment = useCreateWorkItemComment()
  const queryClient = useQueryClient()
  const files = useTaskFiles(item.projectId, item.id)
  const commentFiles = new Map<string, FileAttachment[]>()
  for (const file of files.data ?? []) {
    if (file.commentId) commentFiles.set(file.commentId, [...(commentFiles.get(file.commentId) ?? []), file])
  }
  return (
    <section aria-labelledby={`activity-${item.id}`} className="aw-worksection">
      <header className="aw-worksection__head">
        <h2 className="aw-worksection__title" id={`activity-${item.id}`}>
          <Icon name="activity" size={16} />
          Activity
        </h2>
      </header>
      {timeline.isPending ? <MessageSkeleton label="Loading activity" rows={2} /> : null}
      {timeline.isError ? <Banner tone="danger">Activity didn’t load.</Banner> : null}
      {timeline.data?.length ? (
        <ol className="aw-activity">
          {[...timeline.data].reverse().map((entry) => {
            const actor = entry.comment?.author ?? entry.activity?.actor
            const name = actor?.displayName ?? 'Someone'
            const kind = actorKind(actor?.kind ?? 'human')
            const when = (
              <time className="aw-activity__time" dateTime={entry.createdAt} title={formatFullDate(entry.createdAt)}>
                {formatDateTime(entry.createdAt)}
              </time>
            )
            if (entry.comment) {
              return (
                <li className="aw-activity__comment" key={entry.sequence}>
                  <Avatar kind={kind} name={name} seed={actor?.id} size={28} />
                  <div className="aw-activity__body">
                    <p className="aw-activity__line">
                      <strong>{name}</strong>
                      <ActorTag kind={kind} />
                      {entry.comment.via ? <span className="aw-activity__via">via {entry.comment.via}</span> : null}
                      {when}
                    </p>
                    <div className="aw-activity__content">
                      <MessageContent
                        actors={entry.comment.mentionedActors.map((mentioned) => ({
                          ...mentioned,
                          kind: mentioned.kind as 'human' | 'external_agent' | 'execution_worker' | 'integration' | 'system',
                          avatarUrl: null,
                        }))}
                        content={entry.comment.content}
                        fallbackPlainText={entry.comment.plainText}
                        workItems={entry.comment.referencedWorkItems}
                      />
                      <Attachments files={commentFiles.get(entry.comment.id) ?? []} />
                    </div>
                  </div>
                </li>
              )
            }
            const changes = formatChanges(entry.activity?.changes, item.estimateUnit)
            return (
              <li className="aw-activity__event" key={entry.sequence}>
                <span aria-hidden="true" className="aw-activity__dot" />
                <div className="aw-activity__eventbody">
                  <p className="aw-activity__line">
                    <strong>{name}</strong>
                    <span aria-hidden="true">·</span>
                    {changes.length === 1 && changes[0].kind === 'field' ? <ChangeLine change={changes[0]} /> : entry.activity?.summary}
                    {entry.activity?.via ? <span className="aw-activity__via">via {entry.activity.via}</span> : null}
                    {when}
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
          })}
        </ol>
      ) : timeline.isSuccess ? (
        <p className="aw-quiet">No activity yet.</p>
      ) : null}
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
            if (attachments?.length) void queryClient.invalidateQueries({ queryKey: queryKeys.workItemFiles(item.projectId, item.id) })
          }}
          placeholder="Add a comment. @mention someone or reference a work item."
          searchActors={async (query) => (await searchMentionableActors(item.projectId, query)).actors}
          searchWorkItems={(query) => searchWorkItemReferences(item.projectId, query)}
          submitLabel="Comment"
          upload={uploadFile({ kind: 'task', projectId: item.projectId, workItemId: item.id, forComment: true })}
        />
      ) : null}
    </section>
  )
}
