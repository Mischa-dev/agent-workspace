import { useState, type FormEvent } from 'react'
import { useMutation, useQuery, useQueryClient } from '@tanstack/react-query'
import { useNavigate } from 'react-router-dom'

import { queryKeys } from '../../api/queryKeys'
import { isApiError } from '../../api/errors'
import { describeError, toastError } from '../../app/mutations'
import { getOnboarding, onboardingKeys, progressText, updateOnboarding } from '../../onboarding/api'
import { roleLabel } from '../../projects/format'
import {
  Banner,
  Button,
  ButtonLink,
  ConfirmDialog,
  DetailList,
  Dialog,
  ErrorState,
  ListSkeleton,
  PageHeader,
  Section,
  SelectField,
  SettingsSkeleton,
  TextField,
  countLabel,
  describeLoadError,
  formatFullDate,
  formatShortDate,
  showToast,
} from '../../ui'
import type { SettingsPageProps } from '../types'
import { defaultWorkspace, useSwitchWorkspace } from '../workspaceChoice'
import {
  getServerVersion,
  getWorkspace,
  leaveWorkspace,
  listWorkspaceMembers,
  renameWorkspace,
  type ServerVersion,
  transferOwnership,
  workspaceKeys,
  type Workspace,
} from './api'

/** Workspace › General: its name, which workspace is your default, and leaving or handing it over. */
export default function GeneralPage({ session, workspace: summary }: SettingsPageProps) {
  const organizationId = summary?.id ?? ''
  const serverVersion = useQuery({ queryKey: ['server-version'], queryFn: getServerVersion, staleTime: Infinity })
  const workspace = useQuery({
    queryKey: workspaceKeys.workspace(organizationId),
    queryFn: () => getWorkspace(organizationId),
    enabled: Boolean(organizationId),
  })
  if (!summary) return null

  return (
    <>
      <PageHeader icon="settings" title="General">
        {summary.name}
      </PageHeader>
      {workspace.isPending ? (
        <SettingsSkeleton label="Loading workspace" />
      ) : workspace.isError ? (
        <ErrorState error={workspace.error} onRetry={() => void workspace.refetch()} title="The workspace didn’t load" />
      ) : (
        <>
          <Section id="workspace-details-heading" title="About this workspace">
            <DetailList
              items={[
                { label: 'Your role', value: roleLabel(workspace.data.role) },
                ...(workspace.data.peopleCount === undefined || workspace.data.agentCount === undefined
                  ? [{ label: 'Members', value: String(workspace.data.memberCount) }]
                  : [
                      { label: 'People', value: String(workspace.data.peopleCount) },
                      { label: 'Agents', value: String(workspace.data.agentCount) },
                    ]),
                { label: 'Projects', value: String(workspace.data.projectCount) },
                {
                  label: 'Created',
                  value: <span title={formatFullDate(workspace.data.createdAt)}>{formatShortDate(workspace.data.createdAt)}</span>,
                },
                ...(serverVersion.data ? [{ label: 'Server version', value: versionLabel(serverVersion.data) }] : []),
              ]}
            />
          </Section>
          {workspace.data.capabilities.rename && <RenameSection workspace={workspace.data} />}
          {summary.role === 'owner' || summary.role === 'admin' ? <SetupSection organizationId={summary.id} /> : null}
          {session.organizations.length > 1 && <DefaultSection current={summary.id} session={session} />}
          {workspace.data.capabilities.transferOwnership && <TransferSection workspace={workspace.data} />}
          <LeaveSection workspace={workspace.data} />
        </>
      )}
    </>
  )
}

/** The setup steps on Home: open them, or bring the card back after hiding it. */
function SetupSection({ organizationId }: { organizationId: string }) {
  const queryClient = useQueryClient()
  const onboarding = useQuery({ queryKey: onboardingKeys.workspace(organizationId), queryFn: () => getOnboarding(organizationId) })
  const show = useMutation({
    mutationFn: () => updateOnboarding(organizationId, { dismissed: false }),
    onSuccess: (updated) => {
      queryClient.setQueryData(onboardingKeys.workspace(organizationId), updated)
      showToast({ tone: 'success', title: 'Setup is back on Home' })
    },
    onError: (reason) => toastError('Setup wasn’t shown', reason),
  })
  const data = onboarding.data
  if (!data || data.complete) return null
  return (
    <Section
      description={
        data.dismissed
          ? `You hid the setup steps from Home. ${progressText(data)}.`
          : `The setup steps show on Home until they’re done or you hide them. ${progressText(data)}.`
      }
      id="workspace-setup-heading"
      title="Setup"
    >
      <div>
        {data.dismissed ? (
          <Button busy={show.isPending} onClick={() => show.mutate()}>
            Show setup
          </Button>
        ) : (
          <ButtonLink to="/welcome">Continue setup</ButtonLink>
        )}
      </div>
    </Section>
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
      if (isApiError(reason) && reason.code === 'version_conflict') {
        const latest = await queryClient.fetchQuery({ queryKey: workspaceKeys.workspace(workspace.id), queryFn: () => getWorkspace(workspace.id) })
        setName(latest.name)
        setError('Someone renamed the workspace while you were here. Its current name is shown; try again if you still want to change it.')
      } else {
        setError(`The workspace wasn’t renamed. ${describeError(reason).message}`)
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

/** Whether the workspace shown is your default, and a button to make it so. */
function DefaultSection({ current, session }: { current: string; session: SettingsPageProps['session'] }) {
  const switchWorkspace = useSwitchWorkspace()
  const isDefault = defaultWorkspace(session)?.id === current
  return (
    <Section
      description="The workspace Messages uses for your direct messages, and the one settings open on."
      id="default-workspace-heading"
      title="Default workspace"
    >
      {isDefault ? (
        <p className="aw-quiet">This is your default workspace.</p>
      ) : (
        <div className="aw-inline-form">
          <Button
            busy={switchWorkspace.isPending}
            icon="star"
            onClick={() =>
              switchWorkspace.mutate(current, {
                onSuccess: () => showToast({ tone: 'success', title: 'Default workspace changed' }),
              })
            }
          >
            Make this my default
          </Button>
        </div>
      )}
    </Section>
  )
}

function TransferSection({ workspace }: { workspace: Workspace }) {
  const queryClient = useQueryClient()
  const [open, setOpen] = useState(false)
  const [actorId, setActorId] = useState('')
  const [typed, setTyped] = useState('')
  const close = () => {
    setOpen(false)
    setActorId('')
    setTyped('')
    transfer.reset()
  }
  const members = useQuery({
    queryKey: workspaceKeys.members(workspace.id),
    queryFn: () => listWorkspaceMembers(workspace.id),
    enabled: open,
  })
  const candidates = (members.data?.members ?? []).filter((member) => member.kind === 'human' && member.enabled && !member.isYou)
  const chosen = candidates.find((member) => member.actorId === actorId) ?? null
  // Handing over a workspace can't be taken back by you, so the name is typed to confirm.
  const confirmed = typed.trim() === workspace.name
  const transfer = useMutation({
    mutationFn: () => transferOwnership(workspace.id, actorId),
    onSuccess: (updated) => {
      setOpen(false)
      setTyped('')
      queryClient.setQueryData(workspaceKeys.workspace(workspace.id), updated)
      showToast({ tone: 'success', title: chosen ? `${chosen.displayName} now owns the workspace` : 'Ownership transferred', description: 'You’re an admin now.' })
      void queryClient.invalidateQueries({ queryKey: queryKeys.session })
      void queryClient.invalidateQueries({ queryKey: workspaceKeys.members(workspace.id) })
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
            <Button onClick={close}>Cancel</Button>
            <Button busy={transfer.isPending} disabled={!chosen || !confirmed} type="submit" variant="danger">
              Transfer ownership
            </Button>
          </>
        }
        dirty={Boolean(actorId || typed)}
        onClose={close}
        onSubmit={(event) => {
          event.preventDefault()
          if (chosen && confirmed) transfer.mutate()
        }}
        open={open}
        title="Transfer ownership"
      >
        {members.isPending ? (
          <ListSkeleton avatars label="Loading members" rows={2} />
        ) : members.isError ? (
          <Banner onRetry={() => void members.refetch()} title="Members didn’t load." tone="danger">
            {describeLoadError(members.error)}
          </Banner>
        ) : candidates.length ? (
          <div className="aw-form">
            <SelectField id="transfer-owner" label="New owner" onChange={(event) => setActorId(event.target.value)} value={actorId}>
              <option value="">Choose a person</option>
              {candidates.map((member) => (
                <option key={member.actorId} value={member.actorId}>
                  {member.displayName} (@{member.handle})
                </option>
              ))}
            </SelectField>
            <TextField
              autoComplete="off"
              hint={`Type ${workspace.name} to confirm.`}
              id="transfer-confirm"
              label="Workspace name"
              onChange={(event) => setTyped(event.target.value)}
              value={typed}
            />
            {transfer.isError ? (
              <Banner tone="danger">Ownership wasn’t transferred. {describeError(transfer.error).message}</Banner>
            ) : null}
          </div>
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
  })
  const leaveError = leave.isError
    ? isApiError(leave.error) && leave.error.status === 409
      ? 'You’re the last owner. Transfer ownership to someone else first.'
      : `You didn’t leave the workspace. ${describeError(leave.error).message}`
    : null
  return (
    <Section
      description="You lose access to every project in it. Someone has to invite you again for you to come back."
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
      <ConfirmDialog
        busy={leave.isPending}
        confirmLabel="Leave workspace"
        error={leaveError}
        onClose={() => {
          setOpen(false)
          leave.reset()
        }}
        onConfirm={() => leave.mutate()}
        open={open}
        title={`Leave ${workspace.name}?`}
      >
        {workspace.projectCount > 0
          ? `You’ll lose access to this workspace and its ${countLabel(workspace.projectCount, 'project')}.`
          : 'You’ll lose access to this workspace.'}{' '}
        Someone has to invite you again for you to come back.
      </ConfirmDialog>
    </Section>
  )
}

function versionLabel({ version, commit }: ServerVersion) {
  return commit === 'dev' ? `${version} (development build)` : `${version} (${commit.slice(0, 7)})`
}
