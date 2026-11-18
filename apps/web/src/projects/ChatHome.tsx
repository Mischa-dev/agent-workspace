import { lazy, Suspense, useState } from 'react'
import { Link, useNavigate } from 'react-router-dom'
import { useQuery } from '@tanstack/react-query'

import { isApiError } from '../api/errors'
import { callOp, opQuery } from '../api/ops'
import { useAuthenticatedSession } from '../auth/session'
import { useWorkspaceHref, useWorkspaceId } from '../app/workspace'
import { useProjectConversationSidebar } from '../conversations/hooks'
import { conversationTitle, useDirectOverview } from '../direct/api'
import {
  Avatar,
  Badge,
  Banner,
  Button,
  ButtonLink,
  Dialog,
  EmptyState,
  Icon,
  ListSkeleton,
  Row,
  RowList,
  Section,
  TextField,
  actorKind,
  describeError,
  relativeDay,
  showToast,
} from '../ui'
import { projectLocationHref } from './locations'

const ChannelCreator = lazy(() => import('../shell/ChannelCreator'))

type Item = {
  id: string
  name: string
  href: string
  unread: boolean
  count: number
  meta: string
  leading: React.ReactNode
  /** For the order: unread first, then the newest. */
  at: number
}

/**
 * Home when only chat is on (C2.40): the conversation list, unread first,
 * with New message, New channel and Invite people. The workspace's channels
 * live in its own space; no project or space picker.
 */
export default function ChatHome() {
  const session = useAuthenticatedSession()
  const workspaceId = useWorkspaceId() ?? undefined
  const href = useWorkspaceHref()
  const navigate = useNavigate()
  const sidebar = useQuery({ ...opQuery('sidebar_list', {}, { workspaceId }), enabled: Boolean(workspaceId), staleTime: 60_000 })
  const spaceId = sidebar.data?.workspaceSpaceId ?? ''
  const channels = useProjectConversationSidebar(spaceId)
  const direct = useDirectOverview()
  const [creating, setCreating] = useState(false)
  const [inviting, setInviting] = useState(false)
  const viewerId = session.user.actorId

  const items: Item[] = [
    ...(channels.data?.channels ?? [])
      .filter((channel) => !channel.archivedAt)
      .map((channel) => ({
        id: channel.id,
        name: `#${channel.name}`,
        href: projectLocationHref(spaceId, { kind: 'channel', conversationId: channel.id }),
        unread: channel.unread && !channel.muted,
        count: channel.mentions,
        meta: channel.unread && !channel.muted ? 'New messages' : 'Channel',
        leading: <Icon name={channel.private ? 'lock' : 'hash'} />,
        at: channel.highestSequence > 0 ? 1 : 0,
      })),
    ...(direct.data?.conversations ?? []).map((conversation) => {
      const others = conversation.members.filter((member) => member.id !== viewerId)
      const first = others[0] ?? conversation.members[0]
      const at = Date.parse(conversation.lastActivityAt) || 0
      return {
        id: conversation.id,
        name: conversationTitle(conversation, viewerId),
        href: href(`/messages/${encodeURIComponent(conversation.id)}`),
        unread: conversation.unread && !conversation.muted,
        count: conversation.unreadMessages,
        meta: relativeDay(conversation.lastActivityAt) ?? '',
        leading: first ? <Avatar kind={actorKind(first.kind)} name={first.displayName} seed={first.id} size={28} /> : <Icon name="message" />,
        at,
      }
    }),
  ].sort((left, right) => Number(right.unread) - Number(left.unread) || right.at - left.at || left.name.localeCompare(right.name))

  const loading = sidebar.isPending || (Boolean(spaceId) && channels.isPending && !channels.isError) || direct.isPending
  // Channels of the workspace's own space need the server to serve them
  // with projects off (K4.25); until it does, the DMs show alone.
  const channelsOff = isApiError(channels.error) && channels.error.status === 403
  const failed = sidebar.isError || (channels.isError && !channelsOff) || direct.isError
  const canCreateChannel = Boolean(spaceId) && channels.data?.capabilities.createChannel

  return (
    <Section
      actions={
        <span className="aw-chat-home__actions">
          <ButtonLink icon="compose" size="sm" to={href('/messages')} variant="primary">
            New message
          </ButtonLink>
          {canCreateChannel ? (
            <Button icon="plus" onClick={() => setCreating(true)} size="sm">
              New channel
            </Button>
          ) : null}
          <Button icon="user-plus" onClick={() => setInviting(true)} size="sm">
            Invite people
          </Button>
        </span>
      }
      id="conversations-heading"
      title="Conversations"
    >
      {loading && !failed ? <ListSkeleton label="Loading your conversations" rows={4} /> : null}
      {failed ? (
        <Banner
          onRetry={() => {
            void sidebar.refetch()
            void channels.refetch()
            void direct.refetch()
          }}
          tone="danger"
        >
          Your conversations didn’t load.
        </Banner>
      ) : null}
      {!loading && !failed && items.length === 0 ? (
        <EmptyState icon="message" title="No conversations yet">
          Start one with New message, or invite people to talk with.
        </EmptyState>
      ) : null}
      {items.length ? (
        <RowList label="Conversations">
          {items.map((item) => (
            <Row
              actions={item.count > 0 ? <Badge count={item.count} /> : <Icon className="aw-row__chev" name="chevron-right" />}
              className={['aw-row--link', item.unread ? 'is-unread' : ''].filter(Boolean).join(' ')}
              key={item.id}
              leading={item.leading}
              meta={item.meta}
              title={
                <Link className="aw-row__link" to={item.href}>
                  {item.name}
                </Link>
              }
            />
          ))}
        </RowList>
      ) : null}
      {creating && spaceId ? (
        <Suspense fallback={null}>
          <ChannelCreator onClose={() => setCreating(false)} onCreated={(to) => navigate(to)} open projectId={spaceId} />
        </Suspense>
      ) : null}
      {inviting ? <InviteDialog onClose={() => setInviting(false)} /> : null}
    </Section>
  )
}

/** Invite people with a link (C2.40), copied or shared from the phone's share sheet. */
function InviteDialog({ onClose }: { onClose: () => void }) {
  const workspaceId = useWorkspaceId() ?? undefined
  const [url, setUrl] = useState('')
  const [busy, setBusy] = useState(false)
  const [error, setError] = useState<string | null>(null)
  const make = () => {
    setBusy(true)
    setError(null)
    callOp('invite_link_create', { expiresInDays: 7 }, { workspaceId })
      .then((made) => setUrl(made.url ?? ''))
      .catch((reason: unknown) => setError(describeError(reason).message))
      .finally(() => setBusy(false))
  }
  const copy = () =>
    navigator.clipboard
      .writeText(url)
      .then(() => showToast({ tone: 'success', title: 'Link copied' }))
      .catch(() => showToast({ tone: 'danger', title: 'The link wasn’t copied', description: 'Select it and copy it yourself.' }))
  const share = () => {
    void navigator.share?.({ title: 'Join me', url }).catch(() => undefined)
  }
  return (
    <Dialog
      footer={
        url ? (
          <>
            {typeof navigator.share === 'function' ? (
              <Button icon="external" onClick={share}>
                Share
              </Button>
            ) : null}
            <Button icon="copy" onClick={() => void copy()} variant="primary">
              Copy link
            </Button>
          </>
        ) : (
          <Button busy={busy} icon="link" onClick={make} variant="primary">
            Make a link
          </Button>
        )
      }
      onClose={onClose}
      open
      title="Invite people"
      width={440}
    >
      <p className="aw-quiet">Anyone with the link can join for the next 7 days. They land in the conversation list.</p>
      {error ? (
        <Banner tone="danger" title="No link was made.">
          {error}
        </Banner>
      ) : null}
      {url ? <TextField id="invite-link" label="Invitation link" onFocus={(event) => event.target.select()} readOnly value={url} /> : null}
    </Dialog>
  )
}
