import { FormEvent, useEffect, useRef, useState } from 'react'
import { useMutation, useQuery, useQueryClient } from '@tanstack/react-query'

import { queryKeys } from '../api/queryKeys'
import { isApiError } from '../api/errors'
import {
  issueInvitation,
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

export function TeamSettings({
  projectId,
  organizationRole,
}: {
  projectId: string
  organizationRole: string
}) {
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
      queryClient.invalidateQueries({
        queryKey: queryKeys.invitations(projectId),
      }),
    ])
  }
  const mutationError = (reason: unknown) => {
    setError(
      isApiError(reason)
        ? reason.message
        : 'The team change could not be completed. Try again.',
    )
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
    mutationFn: ({ role, hours }: { role: Exclude<TeamRole, 'owner'>; hours: number }) =>
      issueInvitation(projectId, role, hours),
    onSuccess: async (issued) => {
      setOneShot(issued)
      await refresh()
    },
    onError: mutationError,
  })
  const revokeMutation = useMutation({
    mutationFn: (invitation: Invitation) =>
      revokeInvitation(invitation.id, invitation.version),
    onSuccess: refresh,
    onError: mutationError,
  })
  const replaceMutation = useMutation({
    mutationFn: (invitation: Invitation) =>
      replaceInvitation(invitation.id, invitation.version),
    onSuccess: async (issued) => {
      setOneShot(issued)
      await refresh()
    },
    onError: mutationError,
  })

  if (members.isPending || invitations.isPending) {
    return <p aria-live="polite">Loading team…</p>
  }
  if (members.isError || invitations.isError) {
    return (
      <div className="error-state" role="alert">
        <p>Team settings could not be loaded.</p>
        <button
          onClick={() => {
            void members.refetch()
            void invitations.refetch()
          }}
          type="button"
        >
          Try again
        </button>
      </div>
    )
  }

  return (
    <div className="team-settings">
      {error && (
        <div className="form-error-summary" ref={errorRef} role="alert" tabIndex={-1}>
          {error}
        </div>
      )}
      {oneShot && (
        <OneShotInvitation
          invitation={oneShot}
          onDismiss={() => setOneShot(null)}
        />
      )}
      <InviteForm
        canInviteAdmin={organizationRole === 'owner'}
        disabled={issueMutation.isPending}
        onSubmit={(role, hours) => {
          setError(null)
          issueMutation.mutate({ role, hours })
        }}
      />

      <h3>Members</h3>
      <div className="team-member-list">
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
      </div>

      <h3>Invitation history</h3>
      {!invitations.data.length && <p>No invitations have been issued.</p>}
      <div className="team-invitation-list">
        {invitations.data.map((invitation) => (
          <article className="team-invitation" key={invitation.id}>
            <div>
              <strong>{invitation.role}</strong>
              <span>{invitation.status}</span>
              <small>Issued by {invitation.issuedByDisplayName}</small>
            </div>
            {invitation.status === 'active' && (
              <div className="row-actions">
                <button
                  disabled={replaceMutation.isPending || revokeMutation.isPending}
                  onClick={() => replaceMutation.mutate(invitation)}
                  type="button"
                >
                  Replace link
                </button>
                <button
                  disabled={replaceMutation.isPending || revokeMutation.isPending}
                  onClick={() => revokeMutation.mutate(invitation)}
                  type="button"
                >
                  Revoke
                </button>
              </div>
            )}
          </article>
        ))}
      </div>
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
    <form className="team-invite-form" onSubmit={submit}>
      <div className="field">
        <label htmlFor="invitation-role">Invitation role</label>
        <select
          disabled={disabled}
          id="invitation-role"
          onChange={(event) => setRole(event.target.value as Exclude<TeamRole, 'owner'>)}
          value={role}
        >
          <option value="member">Member</option>
          <option value="viewer">Viewer</option>
          {canInviteAdmin && <option value="admin">Admin</option>}
        </select>
      </div>
      <div className="field">
        <label htmlFor="invitation-expiry">Expires in</label>
        <select
          disabled={disabled}
          id="invitation-expiry"
          onChange={(event) => setHours(Number(event.target.value))}
          value={hours}
        >
          <option value={24}>24 hours</option>
          <option value={72}>3 days</option>
          <option value={168}>7 days</option>
        </select>
      </div>
      <button className="primary-button" disabled={disabled} type="submit">
        {disabled ? 'Creating…' : 'Create invitation link'}
      </button>
    </form>
  )
}

function OneShotInvitation({
  invitation,
  onDismiss,
}: {
  invitation: IssuedInvitation
  onDismiss: () => void
}) {
  const [copied, setCopied] = useState(false)
  return (
    <aside aria-labelledby="invitation-ready-heading" className="one-shot-secret">
      <h3 id="invitation-ready-heading">Invitation link ready</h3>
      <p>Copy it now. The plaintext link cannot be recovered after this panel closes.</p>
      <code>{invitation.invitationUrl}</code>
      <div className="row-actions">
        <button
          onClick={() => {
            void navigator.clipboard.writeText(invitation.invitationUrl).then(() => setCopied(true))
          }}
          type="button"
        >
          {copied ? 'Copied' : 'Copy link'}
        </button>
        <button onClick={onDismiss} type="button">Done</button>
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
    <article className="team-member">
      <div>
        <strong>{member.displayName}</strong>
        <span>@{member.handle}</span>
        <small>
          {member.enabled ? 'Active' : 'Disabled'} · Profile{' '}
          {member.executionProfileReadiness.replace('_', ' ')}
        </small>
      </div>
      <div className="row-actions">
        <label>
          <span className="sr-only">Role for {member.displayName}</span>
          <select
            aria-label={`Role for ${member.displayName}`}
            disabled={disabled || !member.capabilities.changeRole || !member.enabled}
            onChange={(event) =>
              onChange({
                expectedVersion: member.version,
                role: event.target.value as TeamRole,
              })
            }
            value={member.role}
          >
            {member.role === 'owner' && <option value="owner">Owner</option>}
            <option value="admin">Admin</option>
            <option value="member">Member</option>
            <option value="viewer">Viewer</option>
          </select>
        </label>
        {member.capabilities.disable && (
          <button
            disabled={disabled}
            onClick={() => onChange({ expectedVersion: member.version, enabled: false })}
            type="button"
          >
            Disable
          </button>
        )}
        {member.capabilities.enable && (
          <button
            disabled={disabled}
            onClick={() => onChange({ expectedVersion: member.version, enabled: true })}
            type="button"
          >
            Re-enable
          </button>
        )}
      </div>
    </article>
  )
}
