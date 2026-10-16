import { useState, type FormEvent } from 'react'
import { useMutation, useQuery, useQueryClient } from '@tanstack/react-query'
import { useNavigate } from 'react-router-dom'

import { queryKeys } from '../../api/queryKeys'
import { isApiError } from '../../api/errors'
import { describeError, toastError } from '../../app/mutations'
import { roleLabel } from '../../projects/format'
import {
  Banner,
  Button,
  DetailList,
  Dialog,
  ErrorState,
  PageHeader,
  Section,
  SelectField,
  SettingsSkeleton,
  TextField,
  formatShortDate,
  showToast,
} from '../../ui'
import type { SettingsPageProps } from '../types'
import { useSwitchWorkspace } from '../workspaceChoice'
import {
  getWorkspace,
  leaveWorkspace,
  listWorkspaceMembers,
  renameWorkspace,
  transferOwnership,
  workspaceKeys,
  type Workspace,
} from './api'

/** Workspace › General: its name, which workspace is your default, and leaving or handing it over. */
export default function GeneralPage({ session, workspace: summary }: SettingsPageProps) {
  const organizationId = summary?.id ?? ''
  const workspace = useQuery({
    queryKey: workspaceKeys.workspace(organizationId),
    queryFn: () => getWorkspace(organizationId),
    enabled: Boolean(organizationId),
  })
  if (!summary) return null

  return (
    <>
      <PageHeader icon="building" title="General">
        {summary.name}
      </PageHeader>
      {workspace.isPending ? (
        <SettingsSkeleton label="Loading workspace" />
      ) : workspace.isError ? (
        <ErrorState action={<Button onClick={() => void workspace.refetch()}>Try again</Button>} title="The workspace didn’t load">
          Check your connection and try again.
        </ErrorState>
      ) : (
        <>
          <Section id="workspace-details-heading" title="About this workspace">
            <DetailList
              items={[
                { label: 'Your role', value: roleLabel(workspace.data.role) },
                { label: 'Members', value: String(workspace.data.memberCount) },
                { label: 'Projects', value: String(workspace.data.projectCount) },
                { label: 'Created', value: formatShortDate(workspace.data.createdAt) },
              ]}
            />
          </Section>
          {workspace.data.capabilities.rename && <RenameSection workspace={workspace.data} />}
          {session.organizations.length > 1 && <DefaultSection current={summary.id} session={session} />}
          {workspace.data.capabilities.transferOwnership && <TransferSection workspace={workspace.data} />}
          <LeaveSection workspace={workspace.data} />
        </>
      )}
    </>
  )
}

function RenameSection({ workspace }: { workspace: Workspace }) {
  const queryClient = useQueryClient()
  const [name, setName] = useState(workspace.name)
  const [error, setError] = useState<string | null>(null)
  const rename = useMutation({
    mutationFn: () => renameWorkspace(workspace.id, name.trim(), workspace.version),
    onSuccess: (renamed) => {
      setError(null)
      queryClient.setQueryData(workspaceKeys.workspace(workspace.id), renamed)
      showToast({ tone: 'success', title: 'Workspace renamed' })
      void queryClient.invalidateQueries({ queryKey: queryKeys.session })
    },
    onError: async (reason) => {
      if (isApiError(reason) && reason.status === 409) {
        const latest = await queryClient.fetchQuery({ queryKey: workspaceKeys.workspace(workspace.id), queryFn: () => getWorkspace(workspace.id) })
        setName(latest.name)
        setError('Someone renamed the workspace while you were here. Its current name is shown; try again if you still want to change it.')
      } else {
        setError(describeError(reason).message)
      }
    },
  })
  const trimmed = name.trim()
  const submit = (event: FormEvent<HTMLFormElement>) => {
    event.preventDefault()
    if (trimmed && trimmed !== workspace.name) rename.mutate()
  }
  return (
    <Section description="Everyone in the workspace sees this name." id="rename-workspace-heading" title="Name">
      <form className="aw-inline-form" onSubmit={submit}>
        <TextField
          id="workspace-name"
          label="Workspace name"
          maxLength={120}
          onChange={(event) => setName(event.target.value)}
          required
          value={name}
        />
        <Button busy={rename.isPending} disabled={!trimmed || trimmed === workspace.name} type="submit" variant="primary">
          Rename
        </Button>
      </form>
      {error && <Banner tone="warning">{error}</Banner>}
    </Section>
  )
}

function DefaultSection({ current, session }: { current: string; session: SettingsPageProps['session'] }) {
  const switchWorkspace = useSwitchWorkspace()
  return (
    <Section
      description="The workspace settings open on, and the one Messages uses for your direct conversations."
      id="default-workspace-heading"
      title="Default workspace"
    >
      <div className="aw-inline-form">
        <SelectField
          disabled={switchWorkspace.isPending}
          id="default-workspace"
          label="Default workspace"
          onChange={(event) =>
            switchWorkspace.mutate(event.target.value, {
              onSuccess: () => showToast({ tone: 'success', title: 'Default workspace changed' }),
            })
          }
          value={current}
        >
          {session.organizations.map((organization) => (
            <option key={organization.id} value={organization.id}>
              {organization.name}
            </option>
          ))}
        </SelectField>
      </div>
    </Section>
  )
}

function TransferSection({ workspace }: { workspace: Workspace }) {
  const queryClient = useQueryClient()
  const [open, setOpen] = useState(false)
  const [actorId, setActorId] = useState('')
  const members = useQuery({
    queryKey: workspaceKeys.members(workspace.id),
    queryFn: () => listWorkspaceMembers(workspace.id),
    enabled: open,
  })
  const candidates = (members.data?.members ?? []).filter((member) => member.kind === 'human' && member.enabled && !member.isYou)
  const chosen = candidates.find((member) => member.actorId === actorId) ?? null
  const transfer = useMutation({
    mutationFn: () => transferOwnership(workspace.id, actorId),
    onSuccess: (updated) => {
      setOpen(false)
      queryClient.setQueryData(workspaceKeys.workspace(workspace.id), updated)
      showToast({ tone: 'success', title: chosen ? `${chosen.displayName} now owns the workspace` : 'Ownership transferred', description: 'You’re an admin now.' })
      void queryClient.invalidateQueries({ queryKey: queryKeys.session })
      void queryClient.invalidateQueries({ queryKey: workspaceKeys.members(workspace.id) })
    },
    onError: (error) => {
      toastError('Couldn’t transfer ownership', error)
    },
  })
  return (
    <Section
      description="Make another person the owner. You stay in the workspace as an admin."
      id="transfer-ownership-heading"
      title="Transfer ownership"
    >
      <div>
        <Button onClick={() => setOpen(true)}>Transfer ownership…</Button>
      </div>
      <Dialog
        description="The new owner can rename the workspace, manage every member and hand ownership on. You become an admin."
        footer={
          <>
            <Button onClick={() => setOpen(false)}>Cancel</Button>
            <Button busy={transfer.isPending} disabled={!chosen} type="submit" variant="danger">
              Transfer ownership
            </Button>
          </>
        }
        onClose={() => setOpen(false)}
        onSubmit={(event) => {
          event.preventDefault()
          if (chosen) transfer.mutate()
        }}
        open={open}
        title="Transfer ownership"
      >
        {members.isPending ? (
          <p className="aw-quiet">Loading members…</p>
        ) : members.isError ? (
          <Banner tone="danger">Members didn’t load.</Banner>
        ) : candidates.length ? (
          <SelectField id="transfer-owner" label="New owner" onChange={(event) => setActorId(event.target.value)} value={actorId}>
            <option value="">Choose a person</option>
            {candidates.map((member) => (
              <option key={member.actorId} value={member.actorId}>
                {member.displayName} (@{member.handle})
              </option>
            ))}
          </SelectField>
        ) : (
          <p className="aw-quiet">There’s nobody else to hand it to yet. Invite someone to the workspace first.</p>
        )}
      </Dialog>
    </Section>
  )
}

function LeaveSection({ workspace }: { workspace: Workspace }) {
  const queryClient = useQueryClient()
  const navigate = useNavigate()
  const [open, setOpen] = useState(false)
  const lastOwner = workspace.role === 'owner' && !workspace.capabilities.leave
  const leave = useMutation({
    mutationFn: () => leaveWorkspace(workspace.id),
    onSuccess: async () => {
      setOpen(false)
      await queryClient.invalidateQueries({ queryKey: queryKeys.session })
      await queryClient.invalidateQueries({ queryKey: queryKeys.projects })
      navigate('/home', { replace: true, state: { notice: `You left ${workspace.name}.` } })
    },
    onError: (error) => {
      setOpen(false)
      if (isApiError(error) && error.status === 409) {
        toastError('You can’t leave yet', new Error('You’re the last owner. Transfer ownership to someone else first.'))
      } else {
        toastError('Couldn’t leave the workspace', error)
      }
    },
  })
  return (
    <Section
      description="You lose access to every Project in it. Someone will have to invite you again to come back."
      id="leave-workspace-heading"
      title="Leave this workspace"
    >
      {lastOwner ? (
        <p className="aw-quiet">You’re the only owner. Transfer ownership before you leave.</p>
      ) : (
        <div>
          <Button disabled={!workspace.capabilities.leave} icon="log-out" onClick={() => setOpen(true)} variant="danger">
            Leave workspace
          </Button>
        </div>
      )}
      <Dialog
        description={`You’ll be removed from ${workspace.name} and all ${workspace.projectCount === 1 ? 'its Project' : `${workspace.projectCount} of its Projects`}.`}
        footer={
          <>
            <Button onClick={() => setOpen(false)}>Cancel</Button>
            <Button busy={leave.isPending} data-autofocus onClick={() => leave.mutate()} variant="danger">
              Leave
            </Button>
          </>
        }
        onClose={() => setOpen(false)}
        open={open}
        title={`Leave ${workspace.name}?`}
      />
    </Section>
  )
}
