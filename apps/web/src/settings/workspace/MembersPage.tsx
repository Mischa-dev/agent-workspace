import { useMemo, useState, type FormEvent } from 'react'
import { useMutation, useQuery, useQueryClient } from '@tanstack/react-query'

import { describeError, toastError } from '../../app/mutations'
import { humanize, roleLabel } from '../../projects/format'
import {
  ActorTag,
  Avatar,
  Banner,
  Button,
  ConfirmDialog,
  ErrorState,
  Icon,
  ListSkeleton,
  PageHeader,
  Row,
  RowList,
  Section,
  SelectField,
  StatusChip,
  TextField,
  countLabel,
  describeLoadError,
  formatFullDate,
  formatShortDate,
  showToast,
  type ChipTone,
} from '../../ui'
import { DEFAULT_INVITATION_HOURS, INVITATION_EXPIRY_OPTIONS } from '../invitationExpiry'
import { OneTimeValue } from '../OneTimeValue'
import type { SettingsPageProps } from '../types'
import {
  createWorkspaceInvitation,
  listWorkspaceInvitations,
  listWorkspaceMembers,
  revokeWorkspaceInvitation,
  updateWorkspaceMember,
  workspaceKeys,
  type WorkspaceInvitation,
  type WorkspaceMember,
} from './api'

const INVITATION_TONES: Record<WorkspaceInvitation['status'], ChipTone> = {
  active: 'success',
  accepted: 'muted',
  revoked: 'muted',
  expired: 'muted',
}

/** Workspace › Members: people and agents in the workspace, their roles, and invitations to join it. */
export default function MembersPage({ workspace }: SettingsPageProps) {
  const organizationId = workspace?.id ?? ''
  const manager = workspace?.role === 'owner' || workspace?.role === 'admin'
  const queryClient = useQueryClient()
  const [search, setSearch] = useState('')
  const [removing, setRemoving] = useState<WorkspaceMember | null>(null)
  const members = useQuery({
    queryKey: workspaceKeys.members(organizationId),
    queryFn: () => listWorkspaceMembers(organizationId),
    enabled: Boolean(organizationId),
  })
  const refresh = () => queryClient.invalidateQueries({ queryKey: workspaceKeys.members(organizationId) })
  const update = useMutation({
    mutationFn: ({ member, change }: { member: WorkspaceMember; change: { role?: 'admin' | 'member'; enabled?: false } }) =>
      updateWorkspaceMember(organizationId, member, change),
    onSuccess: (updated, { change, member }) => {
      setRemoving(null)
      showToast({
        tone: 'success',
        title:
          change.enabled === false
            ? `${member.displayName} removed from the workspace`
            : `${updated.displayName} is now ${updated.role === 'admin' ? 'an admin' : 'a member'}`,
        onUndo:
          change.role && (member.role === 'admin' || member.role === 'member')
            ? () => update.mutate({ member: updated, change: { role: member.role as 'admin' | 'member' } })
            : undefined,
      })
    },
    onError: (error, { change, member }) => {
      // A failed removal shows in its dialog; a role change says so in a toast.
      if (change.enabled !== false) toastError(`${member.displayName}’s role wasn’t changed`, error)
    },
    onSettled: refresh,
  })

  const shown = useMemo(() => {
    const terms = search.trim().toLocaleLowerCase()
    return (members.data?.members ?? [])
      .filter((member) => member.enabled)
      .filter(
        (member) =>
          !terms ||
          member.displayName.toLocaleLowerCase().includes(terms) ||
          member.handle.toLocaleLowerCase().includes(terms),
      )
      .sort((left, right) => Number(right.isYou) - Number(left.isYou) || left.displayName.localeCompare(right.displayName))
  }, [members.data, search])

  if (!workspace) return null
  const total = members.data?.members.filter((member) => member.enabled).length ?? 0

  return (
    <>
      <PageHeader icon="users" title="Members">
        People and agents in {workspace.name}. Anyone here can be added to any of its projects.
      </PageHeader>
      {manager && <InviteSection organizationId={organizationId} role={workspace.role} />}
      <Section
        actions={total > 8 ? (
          <TextField
            aria-label="Find a member"
            className="aw-settings__filter"
            onChange={(event) => setSearch(event.target.value)}
            placeholder="Find a member"
            prefix={<Icon name="search" size={14} />}
            type="search"
            value={search}
          />
        ) : null}
        id="workspace-members-heading"
        title={total ? `Members (${total})` : 'Members'}
      >
        {members.isPending ? (
          <ListSkeleton avatars label="Loading members" rows={4} />
        ) : members.isError ? (
          <ErrorState error={members.error} onRetry={() => void members.refetch()} title="Members didn’t load" />
        ) : shown.length ? (
          <RowList label="Workspace members">
            {shown.map((member) => (
              <Row
                actions={
                  <>
                    {member.capabilities.changeRole && member.role !== 'owner' ? (
                      <SelectField
                        aria-label={`Role for ${member.displayName}`}
                        className="aw-field--inline"
                        disabled={update.isPending}
                        onChange={(event) =>
                          update.mutate({ member, change: { role: event.target.value as 'admin' | 'member' } })
                        }
                        value={member.role}
                      >
                        {((member.kind === 'human' && workspace.role === 'owner') || member.role === 'admin') && (
                          <option value="admin">Admin</option>
                        )}
                        <option value="member">Member</option>
                      </SelectField>
                    ) : (
                      <StatusChip tone={member.role === 'owner' ? 'agent' : 'muted'}>{roleLabel(member.role)}</StatusChip>
                    )}
                    {member.capabilities.remove && !member.isYou && (
                      <Button
                        disabled={update.isPending}
                        icon="circle-minus"
                        onClick={() => setRemoving(member)}
                        size="sm"
                        variant="ghost"
                      >
                        Remove
                      </Button>
                    )}
                  </>
                }
                key={member.actorId}
                leading={
                  <Avatar
                    kind={member.kind === 'external_agent' ? 'agent' : 'person'}
                    name={member.displayName}
                    seed={member.actorId}
                    size={32}
                  />
                }
                meta={
                  <span className="aw-row__metaline aw-wrap">
                    <span>@{member.handle}</span>
                    <span>In {countLabel(member.projectCount, 'project')}</span>
                    <span title={formatFullDate(member.joinedAt)}>Joined {formatShortDate(member.joinedAt)}</span>
                  </span>
                }
                title={
                  <span className="aw-row__titleline">
                    {member.displayName}
                    {member.kind === 'external_agent' && <ActorTag kind="agent" />}
                    {member.isYou && <StatusChip>You</StatusChip>}
                  </span>
                }
              />
            ))}
          </RowList>
        ) : (
          <p className="aw-quiet" role="status">
            {search.trim() ? `No members match “${search.trim()}”.` : 'No members yet.'}
          </p>
        )}
      </Section>
      <ConfirmDialog
        busy={update.isPending}
        confirmLabel="Remove from workspace"
        error={
          update.isError && update.variables?.change.enabled === false
            ? `${update.variables.member.displayName} wasn’t removed. ${describeError(update.error).message}`
            : null
        }
        onClose={() => {
          setRemoving(null)
          update.reset()
        }}
        onConfirm={() => removing && update.mutate({ member: removing, change: { enabled: false } })}
        open={removing !== null}
        title={removing ? `Remove ${removing.displayName}?` : 'Remove member?'}
      >
        {removing
          ? `${removing.displayName} will be removed from ${workspace.name} and from every project in it. ${
              removing.kind === 'external_agent' ? 'Its credentials stop working.' : 'They’ll need a new invitation to come back.'
            }`
          : null}
      </ConfirmDialog>
    </>
  )
}

function InviteSection({ organizationId, role }: { organizationId: string; role: string }) {
  const queryClient = useQueryClient()
  const invitations = useQuery({
    queryKey: workspaceKeys.invitations(organizationId),
    queryFn: () => listWorkspaceInvitations(organizationId),
  })
  const [inviteRole, setInviteRole] = useState<'admin' | 'member'>('member')
  const [hours, setHours] = useState(DEFAULT_INVITATION_HOURS)
  const [revoking, setRevoking] = useState<WorkspaceInvitation | null>(null)
  const [note, setNote] = useState('')
  const [error, setError] = useState<string | null>(null)
  const [issued, setIssued] = useState<string | null>(null)
  const refresh = () => queryClient.invalidateQueries({ queryKey: workspaceKeys.invitations(organizationId) })
  const create = useMutation({
    mutationFn: () =>
      createWorkspaceInvitation(organizationId, { role: inviteRole, expiresInHours: hours, note: note.trim() || undefined }),
    onSuccess: (invitation) => {
      setIssued(invitation.invitationUrl)
      setNote('')
      setError(null)
    },
    onError: (reason) => setError(`The invitation wasn’t created. ${describeError(reason).message}`),
    onSettled: refresh,
  })
  const revoke = useMutation({
    mutationFn: revokeWorkspaceInvitation,
    onSuccess: () => {
      setRevoking(null)
      showToast({ tone: 'success', title: 'Invitation revoked', description: 'Its link no longer works.' })
    },
    onSettled: refresh,
  })
  const submit = (event: FormEvent<HTMLFormElement>) => {
    event.preventDefault()
    create.mutate()
  }
  const recent = (invitations.data?.invitations ?? []).slice(0, 20)

  return (
    <Section
      description="One link for one person. They join the workspace once and can then be added to any project."
      id="workspace-invite-heading"
      title="Invite to the workspace"
    >
      {issued && (
        <OneTimeValue copyLabel="Copy link" icon="link" onDone={() => setIssued(null)} title="Invitation link ready" value={issued}>
          Copy it now and send it to the person you’re inviting. It can’t be shown again after you close this.
        </OneTimeValue>
      )}
      <form className="aw-card aw-formcard aw-form" onSubmit={submit}>
        <div className="aw-form__grid">
          <SelectField id="workspace-invite-role" label="Role" onChange={(event) => setInviteRole(event.target.value as 'admin' | 'member')} value={inviteRole}>
            <option value="member">Member</option>
            {role === 'owner' && <option value="admin">Admin</option>}
          </SelectField>
          <SelectField id="workspace-invite-expiry" label="Link works for" onChange={(event) => setHours(Number(event.target.value))} value={hours}>
            {INVITATION_EXPIRY_OPTIONS.map((option) => (
              <option key={option.hours} value={option.hours}>
                {option.label}
              </option>
            ))}
          </SelectField>
        </div>
        <TextField
          hint="Only people who manage the workspace see it, to remember who the link is for."
          id="workspace-invite-note"
          label="Note (optional)"
          maxLength={200}
          onChange={(event) => setNote(event.target.value)}
          value={note}
        />
        {error && <Banner tone="danger">{error}</Banner>}
        <div className="aw-form__actions">
          <Button busy={create.isPending} icon="user-plus" type="submit" variant="primary">
            Create invitation link
          </Button>
        </div>
      </form>
      {invitations.isError ? (
        <Banner onRetry={() => void invitations.refetch()} title="Invitations didn’t load." tone="danger">
          {describeLoadError(invitations.error)}
        </Banner>
      ) : recent.length ? (
        <RowList label="Workspace invitations">
          {recent.map((invitation) => (
            <Row
              actions={
                invitation.status === 'active' ? (
                  <Button icon="x" onClick={() => setRevoking(invitation)} size="sm" variant="ghost">
                    Revoke
                  </Button>
                ) : null
              }
              key={invitation.id}
              leading={
                <span className="aw-row__tile">
                  <Icon name="link" size={16} />
                </span>
              }
              meta={
                <span className="aw-row__metaline aw-wrap">
                  <span>By {invitation.issuedByDisplayName}</span>
                  {invitation.status === 'active' && (
                    <span title={formatFullDate(invitation.expiresAt)}>Expires {formatShortDate(invitation.expiresAt)}</span>
                  )}
                  {invitation.status === 'accepted' && invitation.acceptedByDisplayName && (
                    <span>Accepted by {invitation.acceptedByDisplayName}</span>
                  )}
                  {invitation.note && <span>{invitation.note}</span>}
                </span>
              }
              title={
                <span className="aw-row__titleline">
                  {roleLabel(invitation.role)} invitation
                  <StatusChip tone={INVITATION_TONES[invitation.status] ?? 'muted'}>{humanize(invitation.status)}</StatusChip>
                </span>
              }
            />
          ))}
        </RowList>
      ) : null}
      <ConfirmDialog
        busy={revoke.isPending}
        confirmLabel="Revoke invitation"
        error={revoke.isError ? `The invitation wasn’t revoked. ${describeError(revoke.error).message}` : null}
        onClose={() => {
          setRevoking(null)
          revoke.reset()
        }}
        onConfirm={() => revoking && revoke.mutate(revoking)}
        open={revoking !== null}
        title="Revoke this invitation?"
      >
        Its link stops working right away. You can make a new one at any time.
      </ConfirmDialog>
    </Section>
  )
}
