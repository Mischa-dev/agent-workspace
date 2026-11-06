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
import { listRoles, roleKeys, type WorkspaceRoleView } from '../settings/workspace/roles'

/** A role picked from a list: a built-in role's key, or a custom role's ID (AWD-74). */
type RoleChoice = { role?: TeamRole; roleId?: string }
const BUILT_IN_ROLES = new Set<string>(['owner', 'admin', 'member', 'viewer'])
function choiceOf(value: string, customRoles: WorkspaceRoleView[]): RoleChoice {
  if (BUILT_IN_ROLES.has(value)) return { role: value as TeamRole }
  const custom = customRoles.find((role) => role.id === value)
  return custom ? { role: custom.baseRole as TeamRole, roleId: custom.id } : {}
}

function CustomRoleOptions({ roles, agent = false }: { roles: WorkspaceRoleView[]; agent?: boolean }) {
  const shown = roles.filter((role) => !agent || role.baseRole === 'member' || role.baseRole === 'viewer')
  return shown.map((role) => (
    <option key={role.id} value={role.id}>
      {role.name}
    </option>
  ))
}

const INVITATION_TONES: Record<string, ChipTone> = { active: 'success', accepted: 'muted', revoked: 'muted', expired: 'muted' }

const PROJECT_EXPIRY_OPTIONS = INVITATION_EXPIRY_OPTIONS.filter((option) => option.hours <= PROJECT_INVITATION_MAX_HOURS)

/** Whether a member's provider account can take runs, whichever harness it signs in to. */
function runReadiness(readiness: string) {
  if (readiness === 'ready') return 'Ready for runs'
  if (readiness === 'not_configured') return 'No account for runs'
  return `Run account ${humanize(readiness).toLocaleLowerCase()}`
}

export function TeamSettings({
  organizationId,
  organizationRole,
  projectId,
}: {
  organizationId?: string
  organizationRole: string
  projectId: string
}) {
  const queryClient = useQueryClient()
  const roles = useQuery({
    queryKey: roleKeys.roles(organizationId ?? ''),
    queryFn: () => listRoles(organizationId ?? ''),
    enabled: Boolean(organizationId),
  })
  // Custom roles you may give: admin-based ones only owners give.
  const customRoles = (roles.data?.roles ?? []).filter(
    (role) => !role.builtIn && role.capabilities.assign && (role.baseRole !== 'admin' || organizationRole === 'owner'),
  )
  // Roles you may invite someone to, built-in ones included: inviting needs
  // "invite members", not "manage members" (the server works it out as the
  // invitation endpoint does).
  const inviteRoles = (roles.data?.roles ?? []).filter((role) => role.capabilities.invite)
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
      input: { expectedVersion: number; role?: TeamRole; roleId?: string; enabled?: boolean }
    }) => updateMember(projectId, actorId, input.roleId ? { expectedVersion: input.expectedVersion, roleId: input.roleId } : input),
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
    mutationFn: ({ choice, hours }: { choice: RoleChoice; hours: number }) =>
      issueInvitation(projectId, (choice.role ?? 'member') as Exclude<TeamRole, 'owner'>, hours, choice.roleId),
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
      <ExistingMemberForm customRoles={customRoles} projectId={projectId} onAdded={refresh} />
      <Section description="Anyone with the link can join as the role you pick, until it expires or is used." title="Invite people">
        {oneShot && (
          <OneTimeValue copyLabel="Copy link" icon="link" onDone={() => setOneShot(null)} title="Invitation link ready" value={oneShot.invitationUrl}>
            Copy it now and send it to the person you’re inviting. It can’t be shown again after you close this.
          </OneTimeValue>
        )}
        <InviteForm
          canInviteAdmin={organizationRole === 'owner'}
          inviteRoles={inviteRoles}
          rolesLoaded={Boolean(roles.data)}
          disabled={issueMutation.isPending}
          onSubmit={(choice, hours) => {
            setError(null)
            issueMutation.mutate({ choice, hours })
          }}
        />
      </Section>

      <Section title="Members">
        <RowList label="Members">
          {members.data.members.map((member) => (
            <MemberRow
              customRoles={customRoles}
              disabled={memberMutation.isPending}
              key={member.actorId}
              member={member}
              roles={roles.data?.roles ?? []}
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
                    {(invitation.roleId && roles.data?.roles.find((role) => role.id === invitation.roleId)?.name) || roleLabel(invitation.role)}{' '}
                    invitation
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
  inviteRoles,
  rolesLoaded,
  disabled,
  onSubmit,
}: {
  canInviteAdmin: boolean
  inviteRoles: WorkspaceRoleView[]
  rolesLoaded: boolean
  disabled: boolean
  onSubmit: (choice: RoleChoice, hours: number) => void
}) {
  // Only roles you may invite to are offered; until the roles load, the
  // plain ones are. It starts on the first one offered.
  const offers = (key: string) => !rolesLoaded || inviteRoles.some((entry) => entry.builtIn && entry.key === key)
  const offersMember = offers('member')
  const offersViewer = offers('viewer')
  const offersAdmin = canInviteAdmin && inviteRoles.some((entry) => entry.builtIn && entry.key === 'admin')
  const customRoles = inviteRoles.filter((entry) => !entry.builtIn && (entry.baseRole !== 'admin' || canInviteAdmin))
  const firstOffered = offersMember
    ? 'member'
    : offersViewer
      ? 'viewer'
      : (customRoles[0]?.id ?? (offersAdmin ? 'admin' : 'member'))
  const [picked, setRole] = useState<string | null>(null)
  const role = picked ?? firstOffered
  const [hours, setHours] = useState(DEFAULT_INVITATION_HOURS)
  const submit = (event: FormEvent<HTMLFormElement>) => {
    event.preventDefault()
    onSubmit(choiceOf(role, customRoles), hours)
  }
  return (
    <form className="aw-inline-form" onSubmit={submit}>
      <SelectField
        disabled={disabled}
        id="invitation-role"
        label="Invitation role"
        onChange={(event) => setRole(event.target.value)}
        value={role}
      >
        {offersMember && <option value="member">Member</option>}
        {offersViewer && <option value="viewer">Viewer</option>}
        {offersAdmin && <option value="admin">Admin</option>}
        <CustomRoleOptions roles={customRoles} />
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
  customRoles,
  member,
  disabled,
  onChange,
  roles,
}: {
  customRoles: WorkspaceRoleView[]
  member: Member
  disabled: boolean
  onChange: (input: { expectedVersion: number; role?: TeamRole; roleId?: string; enabled?: boolean }) => void
  roles: WorkspaceRoleView[]
}) {
  // A custom role they hold that you can't give still shows as theirs.
  const held = member.roleId ? roles.find((role) => role.id === member.roleId) : undefined
  const options = held && !customRoles.some((role) => role.id === held.id) ? [held, ...customRoles] : customRoles
  return (
    <Row
      actions={
        <>
          <SelectField
            aria-label={`Role for ${member.displayName}`}
            className="aw-field--inline"
            disabled={disabled || !member.capabilities.changeRole || !member.enabled}
            onChange={(event) => onChange({ expectedVersion: member.version, ...choiceOf(event.target.value, options) })}
            value={member.roleId ?? member.role}
          >
            {member.role === 'owner' && <option value="owner">Owner</option>}
            {member.actorKind !== 'external_agent' && <option value="admin">Admin</option>}
            <option value="member">Member</option>
            <option value="viewer">Viewer</option>
            <CustomRoleOptions agent={member.actorKind === 'external_agent'} roles={options} />
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

function ExistingMemberForm({
  customRoles,
  projectId,
  onAdded,
}: {
  customRoles: WorkspaceRoleView[]
  projectId: string
  onAdded: () => Promise<void>
}) {
  const candidates = useQuery({ queryKey: ['member-candidates', projectId], queryFn: () => listMemberCandidates(projectId) })
  const [actorId, setActorId] = useState('')
  const [role, setRole] = useState<string>('member')
  const chosen = (candidates.data ?? []).find((member) => member.actorId === actorId)
  const addable = customRoles.filter((entry) => entry.baseRole === 'member' || entry.baseRole === 'viewer')
  const mutation = useMutation({
    mutationFn: () => {
      const choice = choiceOf(role, addable)
      return addExistingMember(projectId, actorId, choice.role === 'viewer' ? 'viewer' : 'member', choice.roleId)
    },
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
          onChange={(event) => setRole(event.target.value)}
          value={role}
        >
          <option value="member">Member</option>
          <option value="viewer">Viewer</option>
          <CustomRoleOptions roles={addable} />
        </SelectField>
        <Button busy={mutation.isPending} disabled={!actorId || mutation.isPending} type="submit">
          Add member
        </Button>
      </form>
    </Section>
  )
}
