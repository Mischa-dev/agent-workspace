import { useQueryClient } from '@tanstack/react-query'
import { useEffect, useRef, useState, type FormEvent } from 'react'

import { toastError } from '../../app/toastError'
import {
  ActorTag,
  Avatar,
  Button,
  ConfirmDialog,
  IconButton,
  Menu,
  StatusChip,
  TextArea,
  actorKind,
  formatFullDate,
  formatRelative,
  showToast,
  type MenuEntry,
} from '../../ui'
import { Markdown } from '../../ui/markdown'
import { memoryKeys } from '../api'
import {
  addComment,
  changeComment,
  decideSuggestion,
  deleteComment,
  discussionKey,
  type CommentAnchor,
  type PageComment,
  type PageDiscussion as Discussion,
  type PageSuggestion,
  type PageThread,
  type SuggestedEdit,
} from './api'
import './discussion.css'

// A page's discussion (AWD-53): edits agents suggested, waiting for someone
// to accept or reject them, and comment threads, each about words in the
// page or the whole page. In the live editor the words a thread is about
// are marked; choosing either one shows the other.

export type PageDiscussionProps = {
  pageId: string
  projectId: string
  discussion: Discussion
  /** Words someone chose to comment on in the editor, waiting for the comment. */
  draft: CommentAnchor | null
  onDraftDone: () => void
  /** The thread shown as chosen, here and in the editor. */
  active: string | null
  onActivate: (id: string | null) => void
}

function When({ at }: { at: string }) {
  return (
    <time className="aw-activity__time" dateTime={at} title={formatFullDate(at)}>
      {formatRelative(at)}
    </time>
  )
}

/** Shows the words a thread is about where they are in the page. */
function showInPage(id: string) {
  const mark = document.querySelector(`[data-thread="${CSS.escape(id)}"]`)
  mark?.scrollIntoView({ behavior: 'smooth', block: 'center' })
}

export default function PageDiscussion({ active, discussion, draft, onActivate, onDraftDone, pageId, projectId }: PageDiscussionProps) {
  const queryClient = useQueryClient()
  const [showResolved, setShowResolved] = useState(false)
  const refresh = () => queryClient.invalidateQueries({ queryKey: discussionKey(projectId, pageId) })
  const pending = discussion.suggestions.filter((suggestion) => suggestion.status === 'pending')
  const decided = discussion.suggestions.filter((suggestion) => suggestion.status !== 'pending')
  const open = discussion.threads.filter((thread) => !thread.resolved)
  const resolved = discussion.threads.filter((thread) => thread.resolved)
  const shown = showResolved ? discussion.threads : open

  // A notification's link names a thread or a suggestion: bring it into view once it's here.
  const arrived = useRef(false)
  useEffect(() => {
    if (arrived.current) return
    const target = window.location.hash.slice(1)
    if (!target) return
    const element = document.getElementById(target)
    if (!element) return
    arrived.current = true
    element.scrollIntoView({ block: 'center' })
    if (target.startsWith('comment-')) onActivate(target.slice('comment-'.length))
  }, [discussion, onActivate])

  // A thread chosen in the editor comes into view here.
  useEffect(() => {
    if (!active) return
    document.getElementById(`comment-${active}`)?.scrollIntoView({ behavior: 'smooth', block: 'nearest' })
  }, [active])

  return (
    <section aria-labelledby="page-discussion" className="aw-discussion">
      <div className="aw-pagesection__head">
        <div>
          <h2 className="aw-pagesection__title" id="page-discussion">
            Discussion
          </h2>
          <p className="aw-pagesection__desc">
            {discussion.canComment
              ? 'Comments on this page, and changes agents suggested to it. Select words in the page to comment on them.'
              : 'Comments on this page, and changes agents suggested to it.'}
          </p>
        </div>
        {resolved.length > 0 ? (
          <div className="aw-pagesection__actions">
            <Button onClick={() => setShowResolved(!showResolved)} size="sm" variant="ghost">
              {showResolved ? 'Hide resolved' : `Show resolved (${resolved.length})`}
            </Button>
          </div>
        ) : null}
      </div>
      {pending.length > 0 ? (
        <ul aria-label="Suggested changes" className="aw-discussion__list">
          {pending.map((suggestion) => (
            <SuggestionCard
              canDecide={discussion.canComment}
              key={suggestion.id}
              onDecided={async () => {
                await queryClient.invalidateQueries({ queryKey: memoryKeys.page(projectId, pageId) })
              }}
              pageId={pageId}
              suggestion={suggestion}
            />
          ))}
        </ul>
      ) : null}
      {discussion.canComment ? (
        <NewComment
          draft={draft}
          key={draft ? `${draft.blockId}:${draft.quote}` : 'page'}
          onDone={(threadId) => {
            onDraftDone()
            if (threadId) onActivate(threadId)
            void refresh()
          }}
          pageId={pageId}
        />
      ) : null}
      {shown.length > 0 ? (
        <ul aria-label="Comments" className="aw-discussion__list">
          {shown.map((thread) => (
            <Thread
              active={active === thread.id}
              canComment={discussion.canComment}
              key={thread.id}
              onActivate={() => {
                onActivate(thread.id)
                showInPage(thread.id)
              }}
              onChanged={refresh}
              pageId={pageId}
              thread={thread}
              viewerId={discussion.viewerId}
            />
          ))}
        </ul>
      ) : pending.length === 0 ? (
        <p className="aw-discussion__empty">No comments yet.</p>
      ) : null}
      {decided.length > 0 ? (
        <details className="aw-discussion__earlier">
          <summary>Earlier suggestions ({decided.length})</summary>
          <ul className="aw-discussion__decided">
            {decided.map((suggestion) => (
              <li key={suggestion.id}>
                <strong>{suggestion.author ?? 'An agent'}</strong>
                {suggestion.note ? `: ${suggestion.note}` : ''} · {decidedLabel(suggestion)} <When at={suggestion.decidedAt ?? suggestion.createdAt} />
              </li>
            ))}
          </ul>
        </details>
      ) : null}
    </section>
  )
}

function decidedLabel(suggestion: PageSuggestion) {
  switch (suggestion.status) {
    case 'accepted':
      return `accepted${suggestion.decidedBy ? ` by ${suggestion.decidedBy}` : ''}${suggestion.appliedVersion ? ` as version ${suggestion.appliedVersion}` : ''}`
    case 'rejected':
      return `rejected${suggestion.decidedBy ? ` by ${suggestion.decidedBy}` : ''}`
    case 'withdrawn':
      return 'withdrawn'
    default:
      return 'waiting'
  }
}

function editLabel(edit: SuggestedEdit) {
  switch (edit.op) {
    case 'replace':
      return 'Changes'
    case 'delete':
      return 'Removes'
    case 'insertBefore':
      return 'Adds before'
    case 'insertAfter':
      return 'Adds after'
    case 'append':
      return 'Adds at the end'
  }
}

function SuggestionCard({
  canDecide,
  onDecided,
  pageId,
  suggestion,
}: {
  canDecide: boolean
  onDecided: () => Promise<void>
  pageId: string
  suggestion: PageSuggestion
}) {
  const [busy, setBusy] = useState<'accept' | 'reject' | null>(null)
  const kind = actorKind(suggestion.authorKind)
  const name = suggestion.author ?? 'An agent'
  const decide = (decision: 'accept' | 'reject') => {
    setBusy(decision)
    void decideSuggestion(pageId, suggestion.id, decision)
      .then(async (result) => {
        showToast({ title: decision === 'accept' ? `${name}’s changes are in` : 'Suggestion rejected', description: decision === 'accept' ? `Saved as version ${result.version}.` : undefined })
        await onDecided()
      })
      .catch((error: unknown) => toastError(decision === 'accept' ? 'Those changes weren’t made' : 'That wasn’t rejected', error))
      .finally(() => setBusy(null))
  }
  return (
    <li className="aw-discussion__item aw-discussion__suggestion" id={`suggestion-${suggestion.id}`}>
      <Avatar kind={kind} name={name} seed={suggestion.authorId} size={28} />
      <div className="aw-activity__body">
        <p className="aw-activity__line">
          <strong>{name}</strong>
          <ActorTag kind={kind} />
          <span>suggested {suggestion.edits.length === 1 ? 'a change' : `${suggestion.edits.length} changes`}</span>
          <When at={suggestion.createdAt} />
        </p>
        {suggestion.note ? <p className="aw-discussion__note">{suggestion.note}</p> : null}
        <ol className="aw-discussion__edits">
          {suggestion.edits.map((edit, index) => (
            <li className="aw-discussion__edit" key={index}>
              <span className="aw-discussion__op">{editLabel(edit)}</span>
              {edit.before && edit.op !== 'insertBefore' && edit.op !== 'insertAfter' ? (
                <div className="aw-discussion__before">
                  <span className="visually-hidden">Now: </span>
                  <Markdown className="aw-doc" source={edit.before} />
                </div>
              ) : null}
              {edit.before && (edit.op === 'insertBefore' || edit.op === 'insertAfter') ? (
                <p className="aw-discussion__context">{edit.before.split('\n')[0].slice(0, 120)}</p>
              ) : null}
              {edit.markdown ? (
                <div className="aw-discussion__after">
                  <span className="visually-hidden">Suggested: </span>
                  <Markdown className="aw-doc" source={edit.markdown} />
                </div>
              ) : null}
            </li>
          ))}
        </ol>
        {canDecide ? (
          <div className="aw-discussion__actions">
            <Button busy={busy === 'accept'} disabled={busy !== null} icon="check" onClick={() => decide('accept')} size="sm" variant="primary">
              Accept
            </Button>
            <Button busy={busy === 'reject'} disabled={busy !== null} onClick={() => decide('reject')} size="sm">
              Reject
            </Button>
          </div>
        ) : null}
      </div>
    </li>
  )
}

function NewComment({ draft, onDone, pageId }: { draft: CommentAnchor | null; onDone: (threadId: string | null) => void; pageId: string }) {
  const [body, setBody] = useState('')
  const [saving, setSaving] = useState(false)
  const field = useRef<HTMLTextAreaElement>(null)
  useEffect(() => {
    if (draft) field.current?.focus()
  }, [draft])
  const submit = (event: FormEvent) => {
    event.preventDefault()
    if (!body.trim()) return
    setSaving(true)
    void addComment(pageId, { body, ...(draft ?? {}) })
      .then((created) => {
        setBody('')
        onDone(created.threadId)
      })
      .catch((error: unknown) => toastError('That comment wasn’t added', error))
      .finally(() => setSaving(false))
  }
  return (
    <form className="aw-discussion__new" onSubmit={submit}>
      {draft ? <blockquote className="aw-discussion__quote">{draft.quote}</blockquote> : null}
      <TextArea
        label={draft ? 'Your comment on these words' : 'Comment on the page'}
        onChange={(event) => setBody(event.target.value)}
        onKeyDown={(event) => {
          if (event.key === 'Enter' && (event.metaKey || event.ctrlKey)) submit(event)
          if (event.key === 'Escape' && draft) onDone(null)
        }}
        ref={field}
        rows={draft ? 3 : 2}
        value={body}
      />
      <div className="aw-discussion__actions">
        <Button busy={saving} disabled={!body.trim()} size="sm" type="submit" variant="primary">
          Comment
        </Button>
        {draft ? (
          <Button onClick={() => onDone(null)} size="sm" variant="ghost">
            Cancel
          </Button>
        ) : null}
      </div>
    </form>
  )
}

function Thread({
  active,
  canComment,
  onActivate,
  onChanged,
  pageId,
  thread,
  viewerId,
}: {
  active: boolean
  canComment: boolean
  onActivate: () => void
  onChanged: () => Promise<void>
  pageId: string
  thread: PageThread
  viewerId: string
}) {
  const [replying, setReplying] = useState(false)
  const [reply, setReply] = useState('')
  const [busy, setBusy] = useState(false)
  const first = thread.comments[0]
  const run = (title: string, change: Promise<unknown>) => {
    setBusy(true)
    return change
      .then(() => onChanged())
      .catch((error: unknown) => {
        toastError(title, error)
      })
      .finally(() => setBusy(false))
  }
  const sendReply = (event: FormEvent) => {
    event.preventDefault()
    if (!reply.trim()) return
    void run('That reply wasn’t added', addComment(pageId, { body: reply, replyTo: thread.id })).then(() => {
      setReply('')
      setReplying(false)
    })
  }
  return (
    <li className={['aw-discussion__item', 'aw-discussion__thread', active ? 'is-active' : '', thread.resolved ? 'is-resolved' : ''].filter(Boolean).join(' ')} id={`comment-${thread.id}`}>
      <div className="aw-discussion__threadbody">
        {thread.quote ? (
          <button className="aw-discussion__quote aw-discussion__quote--link" onClick={onActivate} type="button">
            {thread.quote}
          </button>
        ) : null}
        {thread.quote && !thread.anchorFound ? <StatusChip>No longer in the page</StatusChip> : null}
        <ul className="aw-discussion__comments">
          {thread.comments.map((comment) => (
            <Comment canComment={canComment} comment={comment} key={comment.id} onChanged={onChanged} own={comment.authorId === viewerId} pageId={pageId} />
          ))}
        </ul>
        {thread.resolved ? (
          <p className="aw-discussion__resolved">
            Resolved{thread.resolvedBy ? ` by ${thread.resolvedBy}` : ''}
            {thread.resolvedAt ? (
              <>
                {' '}
                <When at={thread.resolvedAt} />
              </>
            ) : null}
          </p>
        ) : null}
        {canComment ? (
          replying ? (
            <form className="aw-discussion__reply" onSubmit={sendReply}>
              <TextArea
                aria-label="Reply"
                autoFocus
                onChange={(event) => setReply(event.target.value)}
                onKeyDown={(event) => {
                  if (event.key === 'Enter' && (event.metaKey || event.ctrlKey)) sendReply(event)
                  if (event.key === 'Escape') setReplying(false)
                }}
                rows={2}
                value={reply}
              />
              <div className="aw-discussion__actions">
                <Button busy={busy} disabled={!reply.trim()} size="sm" type="submit" variant="primary">
                  Reply
                </Button>
                <Button onClick={() => setReplying(false)} size="sm" variant="ghost">
                  Cancel
                </Button>
              </div>
            </form>
          ) : (
            <div className="aw-discussion__actions">
              <Button icon="message" onClick={() => setReplying(true)} size="sm" variant="ghost">
                Reply
              </Button>
              <Button
                busy={busy}
                icon={thread.resolved ? 'retry' : 'check'}
                onClick={() => void run(thread.resolved ? 'That wasn’t reopened' : 'That wasn’t resolved', changeComment(pageId, first.id, { resolved: !thread.resolved }))}
                size="sm"
                variant="ghost"
              >
                {thread.resolved ? 'Reopen' : 'Resolve'}
              </Button>
            </div>
          )
        ) : null}
      </div>
    </li>
  )
}

function Comment({
  canComment,
  comment,
  onChanged,
  own,
  pageId,
}: {
  canComment: boolean
  comment: PageComment
  onChanged: () => Promise<void>
  own: boolean
  pageId: string
}) {
  const [editing, setEditing] = useState(false)
  const [text, setText] = useState(comment.body)
  const [saving, setSaving] = useState(false)
  const [deleting, setDeleting] = useState(false)
  const [removing, setRemoving] = useState(false)
  const kind = actorKind(comment.authorKind)
  const name = comment.author ?? 'Someone'
  if (comment.deleted) {
    return (
      <li className="aw-discussion__comment is-deleted">
        <p className="aw-discussion__note">This comment was deleted.</p>
      </li>
    )
  }
  const actions: MenuEntry[] =
    own && canComment
      ? [
          { label: 'Edit', icon: 'pencil', onSelect: () => setEditing(true) },
          { label: 'Delete', icon: 'trash', danger: true, onSelect: () => setDeleting(true) },
        ]
      : []
  const save = (event: FormEvent) => {
    event.preventDefault()
    if (!text.trim()) return
    setSaving(true)
    void changeComment(pageId, comment.id, { body: text })
      .then(async () => {
        setEditing(false)
        await onChanged()
      })
      .catch((error: unknown) => toastError('That comment wasn’t saved', error))
      .finally(() => setSaving(false))
  }
  return (
    <li className="aw-discussion__comment">
      <Avatar kind={kind} name={name} seed={comment.authorId} size={24} />
      <div className="aw-discussion__commentbody">
        <p className="aw-activity__line">
          <strong>{name}</strong>
          <ActorTag kind={kind} />
          {comment.via ? <span className="aw-activity__via">via {comment.via}</span> : null}
          <When at={comment.createdAt} />
          {comment.editedAt ? <span className="aw-activity__via">edited</span> : null}
          {actions.length > 0 && !editing ? (
            <span className="aw-discussion__menu">
              <Menu items={actions} label="Comment actions" placement="bottom-end" trigger={<IconButton icon="more" label="Comment actions" size="sm" />} />
            </span>
          ) : null}
        </p>
        {editing ? (
          <form className="aw-discussion__reply" onSubmit={save}>
            <TextArea aria-label="Edit comment" autoFocus onChange={(event) => setText(event.target.value)} rows={2} value={text} />
            <div className="aw-discussion__actions">
              <Button busy={saving} disabled={!text.trim()} size="sm" type="submit" variant="primary">
                Save
              </Button>
              <Button
                onClick={() => {
                  setEditing(false)
                  setText(comment.body)
                }}
                size="sm"
                variant="ghost"
              >
                Cancel
              </Button>
            </div>
          </form>
        ) : (
          <Markdown className="aw-discussion__text" source={comment.body} />
        )}
      </div>
      <ConfirmDialog
        busy={removing}
        confirmLabel="Delete comment"
        onClose={() => setDeleting(false)}
        onConfirm={() => {
          setRemoving(true)
          void deleteComment(pageId, comment.id)
            .then(async () => {
              setDeleting(false)
              await onChanged()
            })
            .catch((error: unknown) => {
              toastError('That comment wasn’t deleted', error)
            })
            .finally(() => setRemoving(false))
        }}
        open={deleting}
        title="Delete this comment?"
      >
        Replies to it stay.
      </ConfirmDialog>
    </li>
  )
}
