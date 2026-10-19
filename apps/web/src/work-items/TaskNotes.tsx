import { useState } from 'react'
import { Ban, User } from 'lucide-react'

import { useAuthenticatedSession } from '../auth/session'
import { Button, TextArea, formatFullDate, formatRelative } from '../ui'
import { Markdown } from '../ui/markdown/Markdown'
import { useReleaseClaim } from './planningHooks'
import { editErrorMessage } from './taskEdits'
import { TaskNotice } from './TaskNotice'
import type { TaskHandoff, WorkItem } from './types'

// The notes under a task's title: who is working on it, why it's blocked, and the last handoff.

/** Who is working on this right now. Agents claim a task before starting it; the claim ends on its own. */
export function ClaimNotice({ item, projectArchived }: { item: WorkItem; projectArchived: boolean }) {
  const session = useAuthenticatedSession()
  const release = useReleaseClaim()
  const claim = item.claim
  if (!claim) return null
  // Releasing is a task write: it needs edit rights and a project that isn't archived.
  const canRelease = !projectArchived && item.capabilities.edit && (item.capabilities.archive || claim.actor.id === session.user.actorId)
  return (
    <TaskNotice
      actions={
        canRelease ? (
          <Button
            busy={release.isPending}
            onClick={() => release.mutate({ projectId: item.projectId, workItemId: item.id, idempotencyKey: crypto.randomUUID() })}
            size="sm"
          >
            Release
          </Button>
        ) : undefined
      }
      icon={claim.actor.kind === 'external_agent' ? 'bot' : User}
      title={`${claim.actor.displayName} is working on this.`}
      tone="agent"
    >
      {claim.note ? `${claim.note} · ` : ''}Claimed until {formatFullDate(claim.expiresAt)}.
      {release.isError ? ` ${editErrorMessage(release.error, 'Releasing it')}` : ''}
    </TaskNotice>
  )
}

/** The note left by whoever stopped working on it last, where the next person starts reading. */
export function HandoffNotice({ handoff }: { handoff: TaskHandoff }) {
  const [open, setOpen] = useState(false)
  const long = handoff.note.length > 320 || handoff.note.split('\n').length > 5
  return (
    <TaskNotice
      actions={
        long ? (
          <Button aria-expanded={open} onClick={() => setOpen(!open)} size="sm" variant="ghost">
            {open ? 'Show less' : 'Show all'}
          </Button>
        ) : undefined
      }
      icon="message"
      title={
        <>
          Handoff from {handoff.actor.displayName}
          <span className="aw-tasknote__meta">
            {' '}
            <time dateTime={handoff.at} title={formatFullDate(handoff.at)}>
              {formatRelative(handoff.at)}
            </time>
            {handoff.via ? ` via ${handoff.via}` : ''}
          </span>
        </>
      }
      tone="muted"
    >
      <Markdown className={long && !open ? 'aw-tasknote__clamp' : undefined} source={handoff.note} />
    </TaskNotice>
  )
}

/** Why the task is blocked, by whom and since when, with Unblock for people who can change it. */
export function BlockedNotice({ blocked, onUnblock }: { blocked: NonNullable<WorkItem['blocked']>; onUnblock?: () => void }) {
  return (
    <TaskNotice
      actions={
        onUnblock ? (
          <Button onClick={onUnblock} size="sm">
            Unblock
          </Button>
        ) : undefined
      }
      icon={Ban}
      title={
        <>
          {blocked.by ? `${blocked.by.displayName} marked this blocked` : 'Marked blocked'}
          <span className="aw-tasknote__meta">
            {' '}
            <time dateTime={blocked.at} title={formatFullDate(blocked.at)}>
              {formatRelative(blocked.at)}
            </time>
          </span>
        </>
      }
      tone="warning"
    >
      {blocked.reason}
    </TaskNotice>
  )
}

/** Marking a task blocked asks what's in the way, under the title where the answer will show. */
export function BlockForm({ onCancel, onSubmit }: { onCancel: () => void; onSubmit: (reason: string) => void }) {
  const [reason, setReason] = useState('')
  return (
    <form
      className="aw-tasknote aw-tasknote--warning aw-blockform"
      onSubmit={(event) => {
        event.preventDefault()
        if (reason.trim()) onSubmit(reason.trim())
      }}
    >
      <TextArea
        autoFocus
        label="What’s in the way?"
        maxLength={500}
        onChange={(event) => setReason(event.target.value)}
        onKeyDown={(event) => {
          if (event.key === 'Escape') {
            event.preventDefault()
            event.stopPropagation()
            onCancel()
          }
          if (event.key === 'Enter' && (event.metaKey || event.ctrlKey) && reason.trim()) {
            event.preventDefault()
            onSubmit(reason.trim())
          }
        }}
        placeholder="Waiting on the API keys from the vendor"
        rows={2}
        value={reason}
      />
      <div className="aw-tasknote__actions">
        <Button disabled={!reason.trim()} size="sm" type="submit" variant="primary">
          Mark blocked
        </Button>
        <Button onClick={onCancel} size="sm" variant="ghost">
          Cancel
        </Button>
      </div>
    </form>
  )
}
