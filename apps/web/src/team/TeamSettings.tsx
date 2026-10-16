import { useEffect, useRef, useState, type FormEvent } from 'react'
import { useMutation, useQuery, useQueryClient } from '@tanstack/react-query'

import { queryKeys } from '../api/queryKeys'
import { isApiError } from '../api/errors'
import { humanize, roleLabel } from '../projects/format'
import {
  Avatar,
  Banner,
  Button,
  ErrorState,
  Icon,
  MessageSkeleton,
  Row,
  RowList,
  Section,
  SelectField,
  StatusChip,
  formatDateTime,
  type ChipTone,
} from '../ui'
import {
  issueInvitation,
  listMemberCandidates,
  addExistingMember,
  listInvitations,
  listMembers,
  replaceInvitation,
  revokeInvitation,
  updateMember,
  type Invitation,
  type IssuedInvitation,
  type Member,
  type TeamRole,
} from './api'

const INVITATION_TONES: Record<string, ChipTone> = { active: 'success', accepted: 'muted', revoked: 'muted', expired: 'muted' }

function formatDate(iso: string | null | undefined) {
  return formatDateTime(iso) || null
}

function codexReadiness(readiness: string) {
  if (readiness === 'ready') return 'Codex connected'
  if (readiness === 'not_configured') return 'Codex not connected'
  return `Codex ${humanize(readiness).toLocaleLowerCase()}`
}

export function TeamSettings({ projectId, organizationRole }: { projectId: string; organizationRole: string }) {
  const queryClient = useQueryClient()
  const members = useQuery({
    queryKey: queryKeys.members(projectId),
    queryFn: () => listMembers(projectId),
  })
  const invitations = useQuery({
    queryKey: queryKeys.invitations(projectId),
    queryFn: () => listInvitations(projectId),
  })
  const [oneShot, setOneShot] = useState<IssuedInvitation | null>(null)
  const [error, setError] = useState<string | null>(null)
  const errorRef = useRef<HTMLDivElement>(null)

  useEffect(() => {
    if (error) errorRef.current?.focus()
  }, [error])

  const refresh = async () => {
    await Promise.all([
      queryClient.invalidateQueries({ queryKey: queryKeys.members(projectId) }),
      queryClient.invalidateQueries({ queryKey: queryKeys.invitations(projectId) }),
    ])
  }
  const mutationError = (reason: unknown) => {
    setError(isApiError(reason) ? reason.message : 'That change to the team didn’t go through. Try again.')
  }
  const memberMutation = useMutation({
    mutationFn: ({
      actorId,
      input,
    }: {
      actorId: string
      input: { expectedVersion: number; role?: TeamRole; enabled?: boolean }
    }) => updateMember(projectId, actorId, input),
    onSuccess: refresh,
    onError: mutationError,
  })
  const issueMutation = useMutation({
    mutationFn: ({ role, hours }: { role: Exclude<TeamRole, 'owner'>; hours: number }) => issueInvitation(projectId, role, hours),
    onSuccess: async (issued) => {
      setOneShot(issued)
      await refresh()
    },
    onError: mutationError,
  })
  const revokeMutation = useMutation({
    mutationFn: (invitation: Invitation) => revokeInvitation(invitation.id, invitation.version),
    onSuccess: refresh,
    onError: mutationError,
  })
  const replaceMutation = useMutation({
    mutationFn: (invitation: Invitation) => replaceInvitation(invitation.id, invitation.version),
    onSuccess: async (issued) => {
      setOneShot(issued)
      await refresh()
    },
    onError: mutationError,
  })

  if (members.isPending || invitations.isPending) {
    return <MessageSkeleton label="Loading members" rows={3} />
  }
  if (members.isError || invitations.isError) {
    return (
      <ErrorState
        action={
          <Button
            onClick={() => {
              void members.refetch()
              void invitations.refetch()
            }}
          >
            Try again
          </Button>
        }
        title="Members didn’t load"
      >
        Check your connection and try again.
      </ErrorState>
    )
  }

  const invitationBusy = replaceMutation.isPending || revokeMutation.isPending
  return (
    <div className="aw-settings-stack">
      {error && (
        <Banner ref={errorRef} tabIndex={-1} tone="danger">
          {error}
        </Banner>
      )}
      <ExistingMemberForm projectId={projectId} onAdded={refresh} />
      <Section description="Anyone with the link can join as the role you pick, until it expires or is used." title="Invite people">
        {oneShot && <OneShotInvitation invitation={oneShot} onDismiss={() => setOneShot(null)} />}
        <InviteForm
          canInviteAdmin={organizationRole === 'owner'}
          disabled={issueMutation.isPending}
          onSubmit={(role, hours) => {
            setError(null)
            issueMutation.mutate({ role, hours })
          }}
        />
      </Section>

      <Section title="Members">
        <RowList label="Members">
          {members.data.members.map((member) => (
            <MemberRow
              disabled={memberMutation.isPending}
              key={member.actorId}
              member={member}
              onChange={(input) => {
                setError(null)
                memberMutation.mutate({ actorId: member.actorId, input })
              }}
            />
          ))}
        </RowList>
      </Section>

      <Section title="Invitation history">
        {!invitations.data.length ? (
          <p className="aw-quiet">No invitations yet.</p>
        ) : (
          <RowList label="Invitations">
            {invitations.data.map((invitation) => (
              <Row
                actions={
                  invitation.status === 'active' ? (
                    <>
                      <Button disabled={invitationBusy} icon="retry" onClick={() => replaceMutation.mutate(invitation)} size="sm" variant="ghost">
                        Replace link
                      </Button>
                      <Button disabled={invitationBusy} icon="x" onClick={() => revokeMutation.mutate(invitation)} size="sm" variant="ghost">
                        Revoke
                      </Button>
                    </>
                  ) : null
                }
                key={invitation.id}
                leading={
                  <span className="aw-row__tile">
                    <Icon name="link" size={16} />
                  </span>
                }
                meta={`Issued by ${invitation.issuedByDisplayName}${
                  invitation.status === 'active' && formatDate(invitation.expiresAt) ? ` · expires ${formatDate(invitation.expiresAt)}` : ''
                }`}
                title={
                  <span className="aw-row__titleline">
                    {roleLabel(invitation.role)} invitation
                    <StatusChip tone={INVITATION_TONES[invitation.status] ?? 'muted'}>{humanize(invitation.status)}</StatusChip>
                  </span>
                }
              />
            ))}
          </RowList>
        )}
      </Section>
    </div>
  )
}

function InviteForm({
  canInviteAdmin,
  disabled,
  onSubmit,
}: {
  canInviteAdmin: boolean
  disabled: boolean
  onSubmit: (role: Exclude<TeamRole, 'owner'>, hours: number) => void
}) {
  const [role, setRole] = useState<Exclude<TeamRole, 'owner'>>('member')
  const [hours, setHours] = useState(24)
  const submit = (event: FormEvent<HTMLFormElement>) => {
    event.preventDefault()
    onSubmit(role, hours)
  }
  return (
    <form className="aw-inline-form" onSubmit={submit}>
      <SelectField
        disabled={disabled}
        id="invitation-role"
        label="Invitation role"
        onChange={(event) => setRole(event.target.value as Exclude<TeamRole, 'owner'>)}
        value={role}
      >
        <option value="member">Member</option>
        <option value="viewer">Viewer</option>
        {canInviteAdmin && <option value="admin">Admin</option>}
      </SelectField>
      <SelectField
        disabled={disabled}
        id="invitation-expiry"
        label="Expires in"
        onChange={(event) => setHours(Number(event.target.value))}
        value={hours}
      >
        <option value={24}>24 hours</option>
        <option value={72}>3 days</option>
        <option value={168}>7 days</option>
      </SelectField>
      <Button busy={disabled} icon="user-plus" type="submit" variant="primary">
        Create invitation link
      </Button>
    </form>
  )
}

function OneShotInvitation({ invitation, onDismiss }: { invitation: IssuedInvitation; onDismiss: () => void }) {
  const [copied, setCopied] = useState(false)
  return (
    <aside aria-labelledby="invitation-ready-heading" className="aw-card aw-token">
      <div className="aw-token__head">
        <Icon name="link" size={18} />
        <h3 id="invitation-ready-heading">Invitation link ready</h3>
      </div>
      <p>Copy it now and send it to the person you’re inviting. It can’t be shown again after you close this.</p>
      <code className="aw-token__value">{invitation.invitationUrl}</code>
      <div className="aw-card__actions">
        <Button
          icon={copied ? 'check' : 'copy'}
          onClick={() => {
            void navigator.clipboard.writeText(invitation.invitationUrl).then(() => setCopied(true))
          }}
          variant="primary"
        >
          {copied ? 'Copied' : 'Copy link'}
        </Button>
        <Button onClick={onDismiss}>Done</Button>
      </div>
    </aside>
  )
}

function MemberRow({
  member,
  disabled,
  onChange,
}: {
  member: Member
  disabled: boolean
  onChange: (input: { expectedVersion: number; role?: TeamRole; enabled?: boolean }) => void
}) {
  return (
    <Row
      actions={
        <>
          <SelectField
            aria-label={`Role for ${member.displayName}`}
            className="aw-field--inline"
            disabled={disabled || !member.capabilities.changeRole || !member.enabled}
            onChange={(event) => onChange({ expectedVersion: member.version, role: event.target.value as TeamRole })}
            value={member.role}
          >
            {member.role === 'owner' && <option value="owner">Owner</option>}
            {member.actorKind !== 'external_agent' && <option value="admin">Admin</option>}
            <option value="member">Member</option>
            <option value="viewer">Viewer</option>
          </SelectField>
          {member.capabilities.disable && (
            <Button
              disabled={disabled}
              icon="circle-minus"
              onClick={() => onChange({ expectedVersion: member.version, enabled: false })}
              size="sm"
              variant="ghost"
            >
              Disable
            </Button>
          )}
          {member.capabilities.enable && (
            <Button
              disabled={disabled}
              icon="circle-check"
              onClick={() => onChange({ expectedVersion: member.version, enabled: true })}
              size="sm"
              variant="ghost"
            >
              Re-enable
            </Button>
          )}
        </>
      }
      className={member.enabled ? undefined : 'is-disabled'}
      leading={<Avatar name={member.displayName} seed={member.actorId} size={32} />}
      meta={`@${member.handle} · ${member.enabled ? (member.actorKind === 'external_agent' ? 'Agent' : codexReadiness(member.executionProfileReadiness)) : 'Disabled'}`}
      title={member.displayName}
    />
  )
}

function ExistingMemberForm({ projectId, onAdded }: { projectId: string; onAdded: () => Promise<void> }) {
  const candidates = useQuery({ queryKey: ['member-candidates', projectId], queryFn: () => listMemberCandidates(projectId) })
  const [actorId, setActorId] = useState('')
  const [role, setRole] = useState<'member' | 'viewer'>('member')
  const mutation = useMutation({
    mutationFn: () => addExistingMember(projectId, actorId, role),
    onSuccess: async () => { setActorId(''); await onAdded(); await candidates.refetch() },
  })
  return <Section title="Add a workspace member" description="Add an existing person or agent. Agents keep their identity, connection and conversations.">
    <form className="aw-form-stack" onSubmit={(event) => { event.preventDefault(); if (actorId) mutation.mutate() }}>
      {candidates.isError && <Banner tone="danger">Members could not be loaded. <Button onClick={() => void candidates.refetch()}>Retry</Button></Banner>}
      {mutation.isError && <Banner tone="danger">{isApiError(mutation.error) ? mutation.error.message : 'Could not add this member.'}</Banner>}
      <SelectField id="existing-member" label="Person or agent" value={actorId} onChange={(event) => setActorId(event.target.value)} disabled={candidates.isPending || mutation.isPending}>
        <option value="">Choose a member</option>
        {(candidates.data ?? []).map((member) => <option key={member.actorId} value={member.actorId}>{member.displayName} (@{member.handle}) · {member.actorKind === 'external_agent' ? 'Agent' : 'Person'}</option>)}
      </SelectField>
      <SelectField id="existing-member-role" label="Project role" value={role} onChange={(event) => setRole(event.target.value as 'member' | 'viewer')}>
        <option value="member">Member</option><option value="viewer">Viewer</option>
      </SelectField>
      <Button type="submit" disabled={!actorId || mutation.isPending} busy={mutation.isPending}>Add member</Button>
    </form>
  </Section>
}
