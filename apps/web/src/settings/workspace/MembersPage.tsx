import { useMemo, useState, type FormEvent } from 'react'
import { useMutation, useQuery, useQueryClient } from '@tanstack/react-query'

import { describeError, toastError } from '../../app/mutations'
import { humanize, roleLabel } from '../../projects/format'
import {
  ActorTag,
  Avatar,
  Banner,
  Button,
  Checkbox,
  ConfirmDialog,
  Dialog,
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
import {
  assignRole,
  listRoles,
  pickerValue,
  roleKeys,
  roleNameFor,
  setAgentLimit,
  workspaceAssignable,
  type PermissionKey,
  type RolesResponse,
  type WorkspaceRoleView,
} from './roles'

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
  const [limiting, setLimiting] = useState<WorkspaceMember | null>(null)
  const [applyToProjects, setApplyToProjects] = useState(true)
  const roles = useQuery({
    queryKey: roleKeys.roles(organizationId),
    queryFn: () => listRoles(organizationId),
    enabled: Boolean(organizationId),
  })
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
  const assign = useMutation({
    mutationFn: ({ member, role }: { member: WorkspaceMember; role: WorkspaceRoleView }) =>
      assignRole(organizationId, member.actorId, { roleId: role.id, applyToProjects, expectedVersion: member.version }),
    onSuccess: (assignment, { member }) => {
      const projects = assignment.projectsChanged ? ` and in ${countLabel(assignment.projectsChanged, 'project')}` : ''
      showToast({ tone: 'success', title: `${member.displayName} is now ${assignment.roleName}${projects}` })
    },
    onError: (error, { member }) => toastError(`${member.displayName}’s role wasn’t changed`, error),
    onSettled: () => {
      void refresh()
      void queryClient.invalidateQueries({ queryKey: roleKeys.roles(organizationId) })
    },
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
      {manager && <InviteSection organizationId={organizationId} role={workspace.role} roles={roles.data} />}
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
        {manager ? (
          <Checkbox
            checked={applyToProjects}
            description="Giving someone a role also gives it to them in each of their projects, except where they’re the owner."
            label="Change their project roles too"
            onChange={setApplyToProjects}
          />
        ) : null}
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
                    {member.capabilities.assignRole && member.role !== 'owner' && roles.data ? (
                      <SelectField
                        aria-label={`Role for ${member.displayName}`}
                        className="aw-field--inline"
                        disabled={assign.isPending}
                        onChange={(event) => {
                          const role = roles.data.roles.find((entry) => entry.id === event.target.value)
                          if (role) assign.mutate({ member, role })
                        }}
                        value={pickerValue(roles.data.roles, member.role, member.roleId)}
                      >
                        {rolesFor(roles.data, member, workspace.role === 'owner').map((role) => (
                          <option key={role.id} value={role.id}>
                            {role.name}
                          </option>
                        ))}
                      </SelectField>
                    ) : (
                      <StatusChip tone={member.role === 'owner' ? 'agent' : 'muted'}>
                        {member.roleName || roleNameFor(roles.data?.roles, member.role, member.roleId) || roleLabel(member.role)}
                      </StatusChip>
                    )}
                    {member.capabilities.limitApps && roles.data ? (
                      <Button icon="bot" onClick={() => setLimiting(member)} size="sm" variant="ghost">
                        {member.isYou ? 'Your apps' : 'Their apps'}
                      </Button>
                    ) : null}
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
                    {member.agentLimit ? <span>Apps limited</span> : null}
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
      {limiting && roles.data ? (
        <AppsDialog
          member={limiting}
          onClose={() => setLimiting(null)}
          onSaved={() => {
            setLimiting(null)
            void refresh()
          }}
          organizationId={organizationId}
          roles={roles.data}
        />
      ) : null}
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

/** The roles a member can be given here: admin-based ones only by owners, and member- or viewer-based ones for agents. */
function rolesFor(data: RolesResponse, member: WorkspaceMember, owner: boolean) {
  return workspaceAssignable(data.roles).filter(
    (role) =>
      role.id === member.roleId ||
      (role.builtIn && role.key === member.role) ||
      ((role.baseRole !== 'admin' || (owner && member.kind === 'human')) && role.capabilities.assign),
  )
}

/** What apps acting for one person may do in the workspace, on top of their role's limit. */
function AppsDialog({
  member,
  onClose,
  onSaved,
  organizationId,
  roles,
}: {
  member: WorkspaceMember
  onClose: () => void
  onSaved: () => void
  organizationId: string
  roles: RolesResponse
}) {
  const [limited, setLimited] = useState(member.agentLimit !== null)
  const [chosen, setChosen] = useState<PermissionKey[]>(
    (member.agentLimit as PermissionKey[] | null) ?? roles.permissions.map((permission) => permission.key),
  )
  const [error, setError] = useState<string | null>(null)
  const save = useMutation({
    mutationFn: () => setAgentLimit(organizationId, member.actorId, limited ? chosen : null),
    onSuccess: () => {
      showToast({ tone: 'success', title: member.isYou ? 'Your apps’ access saved' : `${member.displayName}’s apps’ access saved` })
      onSaved()
    },
    onError: (reason) => setError(`It wasn’t saved. ${describeError(reason).message}`),
  })
  const choosable = roles.permissions.filter((permission) => !permission.required)
  return (
    <Dialog
      description={
        member.isYou
          ? 'Apps you connect, and agents working for you, can do what you can unless you say otherwise. Their role’s limit applies as well.'
          : `Apps ${member.displayName} connects, and agents working for them, can do what they can unless you say otherwise. Their role’s limit applies as well.`
      }
      footer={
        <>
          <Button onClick={onClose} variant="ghost">
            Cancel
          </Button>
          <Button busy={save.isPending} type="submit" variant="primary">
            Save
          </Button>
        </>
      }
      onClose={onClose}
      onSubmit={(event) => {
        event.preventDefault()
        save.mutate()
      }}
      open
      title={member.isYou ? 'What your apps can do' : `What ${member.displayName}’s apps can do`}
      width={560}
    >
      <div className="aw-form">
        <Checkbox checked={limited} label="Allow less than the person can do" onChange={setLimited} />
        {limited
          ? choosable.map((permission) => (
              <Checkbox
                checked={chosen.includes(permission.key)}
                description={permission.meaning}
                key={permission.key}
                label={permission.label}
                onChange={(on) =>
                  setChosen((current) => (on ? [...new Set([...current, permission.key])] : current.filter((key) => key !== permission.key)))
                }
              />
            ))
          : null}
        <p className="aw-quiet">Reading what they can open always stays allowed.</p>
        {error ? <Banner tone="danger">{error}</Banner> : null}
      </div>
    </Dialog>
  )
}

function InviteSection({ organizationId, role, roles }: { organizationId: string; role: string; roles: RolesResponse | undefined }) {
  const queryClient = useQueryClient()
  const invitations = useQuery({
    queryKey: workspaceKeys.invitations(organizationId),
    queryFn: () => listWorkspaceInvitations(organizationId),
  })
  // Roles you may invite someone to (the server works it out as the
  // invitation endpoint does): inviting needs "invite members", not
  // "manage members". Until the roles load, Member is offered.
  const invitable = (roles?.roles ?? []).filter((entry) => entry.capabilities.invite)
  const offersMember = !roles || invitable.some((entry) => entry.builtIn && entry.key === 'member')
  const offersAdmin = role === 'owner' && invitable.some((entry) => entry.builtIn && entry.key === 'admin')
  const customRoles = invitable.filter((entry) => !entry.builtIn && (entry.baseRole !== 'admin' || role === 'owner'))
  // A built-in role's key, or a custom role's ID. Starts with the workspace's
  // default role when you may invite to it, else the first role you may.
  const defaultRole = roles?.roles.find((entry) => entry.isDefault)
  const [picked, setInviteRole] = useState<string | null>(null)
  const firstInvitable = offersMember ? 'member' : (customRoles[0]?.id ?? (offersAdmin ? 'admin' : 'member'))
  const inviteRole =
    picked ??
    (defaultRole && !defaultRole.builtIn && customRoles.some((entry) => entry.id === defaultRole.id)
      ? defaultRole.id
      : firstInvitable)
  const [hours, setHours] = useState(DEFAULT_INVITATION_HOURS)
  const [revoking, setRevoking] = useState<WorkspaceInvitation | null>(null)
  const [note, setNote] = useState('')
  const [error, setError] = useState<string | null>(null)
  const [issued, setIssued] = useState<string | null>(null)
  const refresh = () => queryClient.invalidateQueries({ queryKey: workspaceKeys.invitations(organizationId) })
  const create = useMutation({
    mutationFn: () =>
      createWorkspaceInvitation(organizationId, {
        role: inviteRole === 'admin' ? 'admin' : 'member',
        roleId: inviteRole === 'admin' || inviteRole === 'member' ? undefined : inviteRole,
        expiresInHours: hours,
        note: note.trim() || undefined,
      }),
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
          <SelectField id="workspace-invite-role" label="Role" onChange={(event) => setInviteRole(event.target.value)} value={inviteRole}>
            {offersMember && <option value="member">Member</option>}
            {offersAdmin && <option value="admin">Admin</option>}
            {customRoles.map((entry) => (
              <option key={entry.id} value={entry.id}>
                {entry.name}
              </option>
            ))}
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
                  {invitation.roleName ?? roleLabel(invitation.role)} invitation
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
