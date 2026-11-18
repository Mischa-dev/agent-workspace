import { useState } from 'react'
import { useQuery, useQueryClient } from '@tanstack/react-query'

import { callOp } from '../api/ops'
import { queryKeys } from '../api/queryKeys'
import { toastError } from '../app/mutations'
import { addGroupMember, removeGroupMember } from '../direct/api'
import { ActorTag, Avatar, Button, ErrorState, IconButton, TextField, actorKind, showToast } from '../ui'
import { searchMentionableActors } from './api'
import type { ConversationActor } from './types'

type Member = { id: string; displayName: string; kind: string }

/**
 * A private channel's members (C2.6): who's in it, and for its managers and
 * the space's moderators, adding and removing people. Anyone in it can leave.
 */
export function ChannelMembers({
  channelId,
  onClose,
  onLeft,
  projectId,
  viewerId,
}: {
  channelId: string
  onClose: () => void
  onLeft: () => void
  projectId: string
  viewerId: string
}) {
  const queryClient = useQueryClient()
  const key = queryKeys.conversationMembers(projectId, channelId)
  const members = useQuery({
    queryKey: key,
    queryFn: () => callOp('conversation_member_list', { conversationId: channelId }, { retries: 0 }),
  })
  const [query, setQuery] = useState('')
  const [busy, setBusy] = useState<string | null>(null)
  const canManage = members.data?.canManage ?? false
  const memberIds = new Set((members.data?.members ?? []).map((member) => member.id))
  const search = query.trim()
  const found = useQuery({
    enabled: canManage && search.length > 0,
    queryKey: queryKeys.mentionableActors(projectId, search),
    queryFn: () => searchMentionableActors(projectId, search),
  })
  const candidates = (found.data?.actors ?? []).filter((actor) => !memberIds.has(actor.id))
  const refresh = () =>
    Promise.all([
      queryClient.invalidateQueries({ queryKey: key }),
      queryClient.invalidateQueries({ queryKey: queryKeys.conversationSidebarRoot(projectId) }),
    ])

  const add = (person: Member | ConversationActor, quiet = false) => {
    setBusy(person.id)
    return addGroupMember(channelId, person.id)
      .then(async () => {
        await refresh()
        setQuery('')
        if (!quiet) showToast({ tone: 'success', title: `Added ${person.displayName}` })
      })
      .catch((reason: unknown) => toastError(`${person.displayName} wasn’t added`, reason))
      .finally(() => setBusy(null))
  }

  // Someone removed can be added back, so it happens at once with Undo.
  const remove = (member: Member) => {
    const leaving = member.id === viewerId
    setBusy(member.id)
    void removeGroupMember(channelId, member.id)
      .then(async () => {
        if (leaving) {
          await queryClient.invalidateQueries({ queryKey: queryKeys.conversationSidebarRoot(projectId) })
          showToast({ tone: 'success', title: 'You left the channel' })
          onLeft()
          return
        }
        await refresh()
        showToast({ tone: 'success', title: `Removed ${member.displayName}`, onUndo: () => add(member, true) })
      })
      .catch((reason: unknown) => toastError(leaving ? 'You didn’t leave' : `${member.displayName} wasn’t removed`, reason))
      .finally(() => setBusy(null))
  }

  return (
    <aside aria-label="Channel members" className="context-drawer aw-members">
      <div className="aw-members__head">
        <h2>Members</h2>
        <IconButton className="aw-panelclose" icon="x" label="Close members" onClick={onClose} size="sm" />
      </div>
      {members.isError ? <ErrorState error={members.error} onRetry={() => void members.refetch()} title="Members didn’t load" /> : null}
      <ul className="aw-members__list">
        {(members.data?.members ?? []).map((member) => {
          const kind = actorKind(member.kind)
          const you = member.id === viewerId
          return (
            <li className="aw-members__row" key={member.id}>
              <Avatar kind={kind} name={member.displayName} seed={member.id} size={32} />
              <span className="aw-members__name">
                {member.displayName}
                {you ? <span className="aw-members__you"> (you)</span> : null}
                {member.manager ? <span className="aw-members__you"> · manages it</span> : null}
              </span>
              <ActorTag kind={kind} />
              {you || canManage ? (
                <Button
                  aria-label={you ? 'Leave this channel' : `Remove ${member.displayName}`}
                  busy={busy === member.id}
                  disabled={busy !== null}
                  onClick={() => remove(member)}
                  size="sm"
                  variant="ghost"
                >
                  {you ? 'Leave' : 'Remove'}
                </Button>
              ) : null}
            </li>
          )
        })}
      </ul>
      {canManage ? (
        <div className="aw-members__add">
          <TextField
            autoComplete="off"
            hint="They’ll see everything said here so far."
            label="Add someone"
            onChange={(event) => setQuery(event.target.value)}
            placeholder="Search people and agents in this space"
            value={query}
          />
          {search ? (
            <ul className="aw-members__list">
              {candidates.map((person) => (
                <li className="aw-members__row" key={person.id}>
                  <Avatar kind={actorKind(person.kind)} name={person.displayName} seed={person.id} size={24} />
                  <span className="aw-members__name">{person.displayName}</span>
                  <Button busy={busy === person.id} disabled={busy !== null} onClick={() => void add(person)} size="sm">
                    Add
                  </Button>
                </li>
              ))}
              {found.isSuccess && candidates.length === 0 ? <li className="aw-members__row">No one else matches.</li> : null}
            </ul>
          ) : null}
        </div>
      ) : null}
    </aside>
  )
}
