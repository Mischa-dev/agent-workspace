import { useEffect, useRef, useState, type FormEvent } from 'react'
import { useMutation, useQuery, useQueryClient } from '@tanstack/react-query'

import { queryKeys } from '../api/queryKeys'
import { humanize, roleLabel } from '../projects/format'
import { OneTimeValue } from '../settings/OneTimeValue'
import { DEFAULT_INVITATION_HOURS, INVITATION_EXPIRY_OPTIONS, PROJECT_INVITATION_MAX_HOURS } from '../settings/invitationExpiry'
import {
  Avatar,
  Banner,
  Button,
  ConfirmDialog,
  ErrorState,
  Icon,
  ListSkeleton,
  Row,
  RowList,
  Section,
  SelectField,
  StatusChip,
  describeError,
  describeLoadError,
  formatFullDate,
  formatShortDate,
  showToast,
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

const PROJECT_EXPIRY_OPTIONS = INVITATION_EXPIRY_OPTIONS.filter((option) => option.hours <= PROJECT_INVITATION_MAX_HOURS)

/** Whether a member's provider account can take runs, whichever harness it signs in to. */
function runReadiness(readiness: string) {
  if (readiness === 'ready') return 'Ready for runs'
  if (readiness === 'not_configured') return 'No account for runs'
  return `Run account ${humanize(readiness).toLocaleLowerCase()}`
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
  const mutationError = (what: string) => (reason: unknown) => {
    setError(`${what} ${describeError(reason).message}`)
  }
  const [revoking, setRevoking] = useState<Invitation | null>(null)
  const memberMutation = useMutation({
    mutationFn: ({
      actorId,
      input,
    }: {
      actorId: string
      input: { expectedVersion: number; role?: TeamRole; enabled?: boolean }
    }) => updateMember(projectId, actorId, input),
    onSuccess: async (updated, { input }) => {
      // Turning access off or on is undone from the toast.
      if (input.enabled !== undefined) {
        showToast({
          tone: 'success',
          title: input.enabled ? `${updated.displayName} can use the project again` : `${updated.displayName} disabled`,
          description: input.enabled ? undefined : 'They keep their history here but can’t open the project.',
          onUndo: () =>
            memberMutation.mutate({ actorId: updated.actorId, input: { expectedVersion: updated.version, enabled: !input.enabled } }),
        })
      }
      await refresh()
    },
    onError: mutationError('That change to the team didn’t go through.'),
  })
  const issueMutation = useMutation({
    mutationFn: ({ role, hours }: { role: Exclude<TeamRole, 'owner'>; hours: number }) => issueInvitation(projectId, role, hours),
    onSuccess: async (issued) => {
      setOneShot(issued)
      await refresh()
    },
    onError: mutationError('The invitation wasn’t created.'),
  })
  const revokeMutation = useMutation({
    mutationFn: (invitation: Invitation) => revokeInvitation(invitation.id, invitation.version),
    onSuccess: async () => {
      setRevoking(null)
      showToast({ tone: 'success', title: 'Invitation revoked', description: 'Its link no longer works.' })
      await refresh()
    },
  })
  const replaceMutation = useMutation({
    mutationFn: (invitation: Invitation) => replaceInvitation(invitation.id, invitation.version, DEFAULT_INVITATION_HOURS),
    onSuccess: async (issued) => {
      setOneShot(issued)
      await refresh()
    },
    onError: mutationError('The link wasn’t replaced.'),
  })

  if (members.isPending || invitations.isPending) {
    return <ListSkeleton label="Loading members" rows={3} avatars />
  }
  if (members.isError || invitations.isError) {
    return (
      <ErrorState
        error={members.error ?? invitations.error}
        onRetry={() => {
          void members.refetch()
          void invitations.refetch()
        }}
        title="Members didn’t load"
      />
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
        {oneShot && (
          <OneTimeValue copyLabel="Copy link" icon="link" onDone={() => setOneShot(null)} title="Invitation link ready" value={oneShot.invitationUrl}>
            Copy it now and send it to the person you’re inviting. It can’t be shown again after you close this.
          </OneTimeValue>
        )}
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
                      <Button disabled={invitationBusy} icon="x" onClick={() => setRevoking(invitation)} size="sm" variant="ghost">
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
                meta={
                  <span className="aw-row__metaline aw-wrap">
                    <span>Issued by {invitation.issuedByDisplayName}</span>
                    {invitation.status === 'active' ? (
                      <span title={formatFullDate(invitation.expiresAt)}>Expires {formatShortDate(invitation.expiresAt)}</span>
                    ) : null}
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
        )}
        <ConfirmDialog
          busy={revokeMutation.isPending}
          confirmLabel="Revoke invitation"
          error={revokeMutation.isError ? `The invitation wasn’t revoked. ${describeError(revokeMutation.error).message}` : null}
          onClose={() => {
            setRevoking(null)
            revokeMutation.reset()
          }}
          onConfirm={() => revoking && revokeMutation.mutate(revoking)}
          open={revoking !== null}
          title="Revoke this invitation?"
        >
          Its link stops working right away. You can make a new one at any time.
        </ConfirmDialog>
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
  const [hours, setHours] = useState(DEFAULT_INVITATION_HOURS)
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
        label="Link works for"
        onChange={(event) => setHours(Number(event.target.value))}
        value={hours}
      >
        {PROJECT_EXPIRY_OPTIONS.map((option) => (
          <option key={option.hours} value={option.hours}>
            {option.label}
          </option>
        ))}
      </SelectField>
      <Button busy={disabled} icon="user-plus" type="submit" variant="primary">
        Create invitation link
      </Button>
    </form>
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
      leading={
        <Avatar
          kind={member.actorKind === 'external_agent' ? 'agent' : 'person'}
          name={member.displayName}
          seed={member.actorId}
          size={32}
        />
      }
      meta={`@${member.handle} · ${
        member.enabled
          ? member.actorKind === 'external_agent'
            ? 'Agent'
            : runReadiness(member.executionProfileReadiness)
          : 'Disabled: keeps their history here, but can’t open the project'
      }`}
      title={member.displayName}
    />
  )
}

function ExistingMemberForm({ projectId, onAdded }: { projectId: string; onAdded: () => Promise<void> }) {
  const candidates = useQuery({ queryKey: ['member-candidates', projectId], queryFn: () => listMemberCandidates(projectId) })
  const [actorId, setActorId] = useState('')
  const [role, setRole] = useState<'member' | 'viewer'>('member')
  const chosen = (candidates.data ?? []).find((member) => member.actorId === actorId)
  const mutation = useMutation({
    mutationFn: () => addExistingMember(projectId, actorId, role),
    onSuccess: async () => {
      if (chosen) showToast({ tone: 'success', title: `Added ${chosen.displayName}` })
      setActorId('')
      await onAdded()
      await candidates.refetch()
    },
  })
  return (
    <Section
      description="Add an existing person or agent. Agents keep their identity, connection and conversations."
      title="Add a workspace member"
    >
      {candidates.isError && (
        <Banner onRetry={() => void candidates.refetch()} title="Workspace members didn’t load." tone="danger">
          {describeLoadError(candidates.error)}
        </Banner>
      )}
      {mutation.isError && <Banner tone="danger">They weren’t added. {describeError(mutation.error).message}</Banner>}
      <form
        className="aw-inline-form"
        onSubmit={(event) => {
          event.preventDefault()
          if (actorId) mutation.mutate()
        }}
      >
        <SelectField
          disabled={candidates.isPending || mutation.isPending}
          id="existing-member"
          label="Person or agent"
          onChange={(event) => setActorId(event.target.value)}
          value={actorId}
        >
          <option value="">Choose a member</option>
          {(candidates.data ?? []).map((member) => (
            <option key={member.actorId} value={member.actorId}>
              {member.displayName} (@{member.handle}) · {member.actorKind === 'external_agent' ? 'Agent' : 'Person'}
            </option>
          ))}
        </SelectField>
        <SelectField
          id="existing-member-role"
          label="Project role"
          onChange={(event) => setRole(event.target.value as 'member' | 'viewer')}
          value={role}
        >
          <option value="member">Member</option>
          <option value="viewer">Viewer</option>
        </SelectField>
        <Button busy={mutation.isPending} disabled={!actorId || mutation.isPending} type="submit">
          Add member
        </Button>
      </form>
    </Section>
  )
}
