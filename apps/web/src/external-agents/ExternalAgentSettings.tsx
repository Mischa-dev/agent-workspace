import { useEffect, useState, type FormEvent } from 'react'
import { useMutation, useQuery, useQueryClient } from '@tanstack/react-query'

import { WorkOsAgentSettings } from './WorkOsAgentSettings'
import { toastError } from '../app/mutations'
import { apiUrl } from '../api/client'
import { queryKeys } from '../api/queryKeys'
import { OneTimeValue } from '../settings/OneTimeValue'
import {
  ActorTag,
  Avatar,
  Banner,
  Button,
  ConfirmDialog,
  Dialog,
  EmptyState,
  ErrorState,
  Icon,
  IconButton,
  ListSkeleton,
  Menu,
  Section,
  SegmentedControl,
  SelectField,
  StatusChip,
  TextField,
  describeError,
  formatFullDate,
  formatRelative,
  formatShortDate,
  formatTime,
  showToast,
  type MenuEntry,
} from '../ui'
import {
  createExternalAgent,
  createExternalAgentSetupLink,
  listExternalAgents,
  removeExternalAgent,
  rotateExternalAgentCredential,
  setExternalAgentWebhook,
  testExternalAgentWebhook,
  updateExternalAgent,
  type ExternalAgentConnection,
  type ExternalAgentMutation,
  type ExternalAgentRole,
  type ExternalAgentScope,
  type OneTimeCredential,
  type SetupApp,
  setupMessage,
} from './api'
import { RUNNER_PRESETS, runnerSettings, type RunnerPresetId } from './runnerPresets'

const memberScopes: ExternalAgentScope[] = ['read', 'write', 'events', 'approval_respond', 'worker_request']
const viewerScopes: ExternalAgentScope[] = ['read', 'events']

const SCOPE_LABELS: Record<ExternalAgentScope, string> = {
  read: 'Read conversations',
  write: 'Post messages',
  events: 'Receive live events',
  approval_respond: 'Answer approvals',
  worker_request: 'Ask for runs',
}

const ROLE_LABELS: Record<ExternalAgentRole, string> = { member: 'Reads and posts', viewer: 'Reads only' }

function RoleOptions() {
  return (
    <>
      <option value="member">{ROLE_LABELS.member}</option>
      <option value="viewer">{ROLE_LABELS.viewer}</option>
    </>
  )
}

type Confirming = { kind: 'rotate' | 'remove' | 'setup'; connection: ExternalAgentConnection }

const SETUP_APPS: { value: SetupApp; label: string }[] = [
  { value: 'claude_code', label: 'Claude Code' },
  { value: 'codex', label: 'Codex' },
  { value: 'other', label: 'Another app' },
]

function SetupAppField({
  disabled,
  id,
  onChange,
  value,
  withToken = false,
}: {
  disabled: boolean
  id: string
  onChange: (value: SetupApp | '') => void
  value: SetupApp | ''
  /** Offer showing the token instead. */
  withToken?: boolean
}) {
  return (
    <SelectField
      disabled={disabled}
      hint="You get one message to paste there. The agent fetches its token and steps itself."
      id={id}
      label="Set it up in"
      onChange={(event) => onChange(event.target.value as SetupApp | '')}
      value={value}
    >
      {SETUP_APPS.map((app) => (
        <option key={app.value} value={app.value}>
          {app.label}
        </option>
      ))}
      {withToken ? <option value="">Show me the token instead</option> : null}
    </SelectField>
  )
}

export function ExternalAgentSettings({
  onBusy,
  projectId,
}: {
  /** True while a change is on its way or a new token is on screen, so the page keeps this place in view. */
  onBusy?: (busy: boolean) => void
  projectId: string
}) {
  const queryClient = useQueryClient()
  const connections = useQuery({
    queryKey: queryKeys.externalAgents(projectId),
    queryFn: () => listExternalAgents(projectId),
  })
  const [oneTimeCredential, setOneTimeCredential] = useState<{
    agentName: string
    credential: OneTimeCredential
    /** Set when the agent was just created: what its runner needs. */
    setup?: { handle: string; runner: RunnerPresetId; webhookSecret: string | null }
  } | null>(null)
  const [setupLink, setSetupLink] = useState<{ agentName: string; message: string; expiresAt: string } | null>(null)
  const [setupApp, setSetupApp] = useState<SetupApp>('claude_code')
  const [creatingAs, setCreatingAs] = useState<RunnerPresetId>('agent')
  const [createError, setCreateError] = useState<string | null>(null)
  const [confirming, setConfirming] = useState<Confirming | null>(null)

  const refresh = async () => {
    await queryClient.invalidateQueries({ queryKey: queryKeys.externalAgents(projectId) })
  }
  const showCredential = (result: ExternalAgentMutation, created = false) => {
    if (result.setupLink) {
      setOneTimeCredential(null)
      setSetupLink({
        agentName: result.connection.displayName,
        message: setupMessage(result.connection.handle, result.setupLink.url),
        expiresAt: result.setupLink.expiresAt,
      })
      return
    }
    if (!result.oneTimeCredential) return
    setSetupLink(null)
    setOneTimeCredential({
      agentName: result.connection.displayName,
      credential: result.oneTimeCredential,
      setup: created ? { handle: result.connection.handle, runner: creatingAs, webhookSecret: result.webhookSecret ?? null } : undefined,
    })
  }
  const createMutation = useMutation({
    mutationFn: (input: Parameters<typeof createExternalAgent>[1]) => createExternalAgent(projectId, input),
    onSuccess: async (result) => {
      showCredential(result, true)
      await refresh()
    },
    onError: (reason) => setCreateError(`The agent wasn’t connected. ${describeError(reason).message}`),
  })
  const enableMutation = useMutation({
    mutationFn: (input: { connection: ExternalAgentConnection; enabled: boolean; undo?: boolean }) =>
      updateExternalAgent(projectId, input.connection.id, { expectedVersion: input.connection.version, enabled: input.enabled }),
    onSuccess: async (result, input) => {
      if (!input.undo) {
        showToast({
          title: input.enabled ? `${result.connection.displayName} turned back on` : `${result.connection.displayName} disabled`,
          description: input.enabled ? undefined : 'It can’t connect until you turn it back on.',
          onUndo: () => enableMutation.mutate({ connection: result.connection, enabled: !input.enabled, undo: true }),
        })
      }
      await refresh()
    },
    onError: (reason, input) => {
      toastError(input.enabled ? `${input.connection.displayName} wasn’t turned back on` : `${input.connection.displayName} wasn’t disabled`, reason)
      void refresh()
    },
  })
  const rotateMutation = useMutation({
    mutationFn: (connection: ExternalAgentConnection) => rotateExternalAgentCredential(projectId, connection.id, connection.version),
    onSuccess: async (result) => {
      setConfirming(null)
      showCredential(result)
      await refresh()
    },
  })
  const setupMutation = useMutation({
    mutationFn: (input: { connection: ExternalAgentConnection; app: SetupApp }) =>
      createExternalAgentSetupLink(projectId, input.connection.id, input.connection.version, input.app),
    onSuccess: async (result) => {
      setConfirming(null)
      showCredential(result)
      await refresh()
    },
  })
  const removeMutation = useMutation({
    mutationFn: (connection: ExternalAgentConnection) => removeExternalAgent(projectId, connection.id, connection.version),
    onSuccess: async (result) => {
      setConfirming(null)
      showToast({ title: `${result.connection.displayName} removed` })
      await refresh()
    },
  })
  const confirmMutation = confirming?.kind === 'rotate' ? rotateMutation : removeMutation
  const pending =
    createMutation.isPending ||
    enableMutation.isPending ||
    rotateMutation.isPending ||
    setupMutation.isPending ||
    removeMutation.isPending
  const holding = pending || oneTimeCredential !== null || setupLink !== null
  useEffect(() => {
    onBusy?.(holding)
  }, [holding, onBusy])
  useEffect(() => () => onBusy?.(false), [onBusy])

  if (connections.isPending) return <ListSkeleton label="Loading agents" rows={2} avatars />
  if (connections.isError) {
    return <ErrorState error={connections.error} onRetry={() => void connections.refetch()} title="Agents didn’t load" />
  }

  return (
    <div className="aw-agents">
      {oneTimeCredential && (
        <OneTimeToken
          agentName={oneTimeCredential.agentName}
          credential={oneTimeCredential.credential}
          onDismiss={() => setOneTimeCredential(null)}
          setup={oneTimeCredential.setup}
        />
      )}
      {setupLink && (
        <OneTimeValue
          copyLabel="Copy message"
          icon="link"
          onDone={() => setSetupLink(null)}
          title={`Setup message ready for ${setupLink.agentName}`}
          value={setupLink.message}
        >
          Paste it into the agent’s app. The link in it works once, until{' '}
          <span title={formatFullDate(setupLink.expiresAt)}>{formatTime(setupLink.expiresAt)}</span>, and it can’t be
          shown again after you close this.
        </OneTimeValue>
      )}
      <Section description="The agents that can read and post here." title="Connected agents">
        {!connections.data.connections.length ? (
          <EmptyState icon="bot" title="No agents connected yet">
            Connect one below. It gets its own token and posts as itself.
          </EmptyState>
        ) : (
          <div className="aw-agents__list">
            {connections.data.connections.map((connection) => (
              <AgentCard
                connection={connection}
                disabled={pending}
                key={connection.id}
                onConfirm={(kind) => {
                  rotateMutation.reset()
                  setupMutation.reset()
                  removeMutation.reset()
                  setConfirming({ kind, connection })
                }}
                onEnable={(enabled) => enableMutation.mutate({ connection, enabled })}
                projectId={projectId}
              />
            ))}
          </div>
        )}
      </Section>
      <Section
        description="A runner answers when people tag it in a conversation (for example @reviewer). Any other agent connects over MCP: you get one setup message to paste into its app, or its token, shown once."
        title="Connect an agent"
      >
        {createError ? (
          <Banner className="aw-agents__error" tone="danger">
            {createError}
          </Banner>
        ) : null}
        <CreateAgentForm
          disabled={pending}
          onSubmit={(preset, input) => {
            setCreateError(null)
            setCreatingAs(preset)
            createMutation.mutate(input)
          }}
        />
      </Section>
      <ConfirmDialog
        busy={confirmMutation.isPending}
        confirmLabel={confirming?.kind === 'rotate' ? 'Make a new token' : 'Remove'}
        error={
          confirmMutation.isError
            ? `${confirming?.kind === 'remove' ? 'It wasn’t removed.' : 'No new token was made.'} ${describeError(confirmMutation.error).message}`
            : null
        }
        onClose={() => setConfirming(null)}
        onConfirm={() => {
          if (!confirming) return
          if (confirming.kind === 'rotate') rotateMutation.mutate(confirming.connection)
          else removeMutation.mutate(confirming.connection)
        }}
        open={confirming !== null && confirming.kind !== 'setup'}
        title={
          confirming?.kind === 'rotate'
            ? `Make a new token for ${confirming.connection.displayName}?`
            : `Remove ${confirming?.connection.displayName ?? 'this agent'}?`
        }
        tone={confirming?.kind === 'remove' ? 'danger' : 'primary'}
      >
        {confirming?.kind === 'rotate'
          ? `${confirming.connection.displayName} stops working until you give it the new one. The old token stops working right away.`
          : 'Its token stops working and it can’t connect here again. What it already posted stays.'}
      </ConfirmDialog>
      <Dialog
        description={
          confirming?.kind === 'setup'
            ? `This gives ${confirming.connection.displayName} a new token. The old one stops working right away.`
            : undefined
        }
        footer={
          <>
            <Button data-autofocus disabled={setupMutation.isPending} onClick={() => setConfirming(null)}>
              Cancel
            </Button>
            <Button busy={setupMutation.isPending} type="submit" variant="primary">
              Make setup message
            </Button>
          </>
        }
        onClose={() => setConfirming(null)}
        onSubmit={(event) => {
          event.preventDefault()
          if (confirming?.kind === 'setup') setupMutation.mutate({ connection: confirming.connection, app: setupApp })
        }}
        open={confirming?.kind === 'setup'}
        title={`Set up ${confirming?.connection.displayName ?? 'this agent'} in an app`}
        width={440}
      >
        <SetupAppField
          disabled={setupMutation.isPending}
          id="external-agent-setup-app"
          onChange={(value) => setSetupApp(value || 'other')}
          value={setupApp}
        />
        {setupMutation.isError ? (
          <Banner tone="danger">{`No new token was made. ${describeError(setupMutation.error).message}`}</Banner>
        ) : null}
      </Dialog>
    </div>
  )
}

function CreateAgentForm({
  disabled,
  onSubmit,
}: {
  disabled: boolean
  onSubmit: (preset: RunnerPresetId, input: Parameters<typeof createExternalAgent>[1]) => void
}) {
  const [preset, setPreset] = useState<RunnerPresetId>('agent')
  const [connectionName, setConnectionName] = useState('')
  const [displayName, setDisplayName] = useState('')
  const [handle, setHandle] = useState('')
  const [webhookUrl, setWebhookUrl] = useState('')
  const [setupApp, setSetupApp] = useState<SetupApp | ''>('claude_code')
  const [role, setRole] = useState<ExternalAgentRole>('member')
  const [scopes, setScopes] = useState<ExternalAgentScope[]>(memberScopes)
  const runner = RUNNER_PRESETS.find((candidate) => candidate.id === preset)?.runner ?? false
  const choosePreset = (next: RunnerPresetId) => {
    const chosen = RUNNER_PRESETS.find((candidate) => candidate.id === next)
    setPreset(next)
    if (!chosen) return
    setDisplayName(chosen.displayName)
    setHandle(chosen.handle)
    setConnectionName(chosen.connectionName)
    if (chosen.runner) {
      // Runners answer people, so they read and post.
      setRole('member')
      setScopes(memberScopes)
    }
  }
  const submit = (event: FormEvent<HTMLFormElement>) => {
    event.preventDefault()
    onSubmit(preset, {
      connectionName,
      displayName,
      handle,
      role,
      scopes,
      ...(runner && webhookUrl.trim() ? { webhookUrl: webhookUrl.trim() } : {}),
      ...(!runner && setupApp ? { setupApp } : {}),
    })
  }
  const changeRole = (nextRole: ExternalAgentRole) => {
    setRole(nextRole)
    setScopes(nextRole === 'viewer' ? viewerScopes : memberScopes)
  }
  return (
    <form className="aw-card aw-agentform" onSubmit={submit}>
      <SegmentedControl
        label="Kind of agent"
        onChange={choosePreset}
        options={RUNNER_PRESETS.map((candidate) => ({
          value: candidate.id,
          label: candidate.label,
          icon: candidate.runner ? 'terminal' : 'bot',
        }))}
        value={preset}
      />
      {runner ? (
        <p className="aw-agentform__hint">
          People tag it and it does the work: it hears a wakeup at your runner’s URL, then claims the request, posts
          progress and answers in the thread.{' '}
          <a href="/agent-runner.md" rel="noreferrer" target="_blank">
            How a runner works
          </a>
        </p>
      ) : null}
      <div className="aw-agentform__grid">
        <TextField
          disabled={disabled}
          hint="How people see it in messages."
          id="external-agent-display-name"
          label="Display name"
          maxLength={80}
          onChange={(event) => setDisplayName(event.target.value)}
          required
          value={displayName}
        />
        <TextField
          disabled={disabled}
          hint="Used for @mentions."
          id="external-agent-handle"
          label="Handle"
          maxLength={32}
          onChange={(event) => setHandle(event.target.value)}
          prefix="@"
          required
          spellCheck={false}
          value={handle}
        />
        <TextField
          disabled={disabled}
          hint="Only you see this, for example the machine it runs on."
          id="external-agent-connection-name"
          label="Connection name"
          maxLength={80}
          onChange={(event) => setConnectionName(event.target.value)}
          required
          value={connectionName}
        />
        {runner ? (
          <TextField
            disabled={disabled}
            hint="Where your runner listens for wakeups. Leave it empty if it checks its inbox on a schedule."
            id="external-agent-webhook"
            label="Runner URL (optional)"
            maxLength={500}
            onChange={(event) => setWebhookUrl(event.target.value)}
            placeholder="https://…"
            spellCheck={false}
            type="url"
            value={webhookUrl}
          />
        ) : (
          <SelectField
            disabled={disabled}
            id="external-agent-role"
            label="Access"
            onChange={(event) => changeRole(event.target.value as ExternalAgentRole)}
            value={role}
          >
            <RoleOptions />
          </SelectField>
        )}
      </div>
      {runner ? null : <ScopeFields disabled={disabled} onChange={setScopes} role={role} scopes={scopes} />}
      {runner ? null : (
        <SetupAppField disabled={disabled} id="external-agent-setup-app-new" onChange={setSetupApp} value={setupApp} withToken />
      )}
      <div className="aw-form__actions">
        <Button busy={disabled} icon="bot" type="submit" variant="primary">
          {runner ? 'Add runner' : 'Connect agent'}
        </Button>
      </div>
    </form>
  )
}

function ScopeFields({
  disabled,
  onChange,
  role,
  scopes,
}: {
  disabled: boolean
  onChange: (scopes: ExternalAgentScope[]) => void
  role: ExternalAgentRole
  scopes: ExternalAgentScope[]
}) {
  const allowed = role === 'viewer' ? viewerScopes : memberScopes
  return (
    <fieldset className="aw-scopes">
      <legend className="aw-scopes__legend">What it can do</legend>
      {allowed.map((scope) => (
        <label className="aw-scope" key={scope}>
          <input
            checked={scopes.includes(scope)}
            disabled={disabled || scope === 'read'}
            onChange={(event) =>
              onChange(event.target.checked ? [...scopes, scope] : scopes.filter((candidate) => candidate !== scope))
            }
            type="checkbox"
          />
          <span className="aw-scope__box" aria-hidden="true">
            <Icon name="check" size={12} />
          </span>
          {SCOPE_LABELS[scope]}
        </label>
      ))}
    </fieldset>
  )
}

function AgentCard({
  connection,
  disabled,
  onConfirm,
  onEnable,
  projectId,
}: {
  connection: ExternalAgentConnection
  disabled: boolean
  onConfirm: (kind: Confirming['kind']) => void
  onEnable: (enabled: boolean) => void
  projectId: string
}) {
  const active = connection.status === 'active'
  const [editing, setEditing] = useState(false)
  const [secret, setSecret] = useState<string | null>(null)
  const menu: MenuEntry[] = [
    ...(active
      ? [
          { label: 'Set up in an app…', icon: 'link' as const, disabled, onSelect: () => onConfirm('setup') },
          { label: 'New token…', icon: 'retry' as const, disabled, onSelect: () => onConfirm('rotate') },
          { label: 'Disable', icon: 'circle-minus' as const, disabled, onSelect: () => onEnable(false) },
          { separator: true as const },
        ]
      : []),
    { label: 'Remove…', icon: 'trash', danger: true, disabled, onSelect: () => onConfirm('remove') },
  ]
  return (
    <article className="aw-card aw-agentcard">
      <header className="aw-agentcard__head">
        <Avatar kind="agent" name={connection.displayName} seed={connection.actorId} size={40} />
        <div className="aw-agentcard__titles">
          <div className="aw-agentcard__name">
            <strong>{connection.displayName}</strong>
            <ActorTag kind="agent" />
          </div>
          <span className="aw-agentcard__sub">
            @{connection.handle} · {ROLE_LABELS[connection.role] ?? ROLE_LABELS.member} ·{' '}
            {connection.lastUsedAt ? (
              <span title={formatFullDate(connection.lastUsedAt)}>last active {formatRelative(connection.lastUsedAt)}</span>
            ) : (
              'not used yet'
            )}
          </span>
        </div>
        <StatusChip icon={active ? 'circle-check' : 'circle-minus'} tone={active ? 'success' : 'muted'}>
          {active ? 'Active' : connection.status === 'removed' ? 'Removed' : 'Disabled'}
        </StatusChip>
      </header>
      <ul aria-label="Permissions" className="aw-agentcard__scopes">
        {connection.scopes.map((scope) => (
          <li key={scope}>{SCOPE_LABELS[scope] ?? scope}</li>
        ))}
      </ul>
      {secret ? (
        <OneTimeValue copyLabel="Copy secret" onDone={() => setSecret(null)} title={`Signing secret for ${connection.displayName}`} value={secret}>
          Give it to the runner now so it can check each wakeup. It can’t be shown again after you close this.
        </OneTimeValue>
      ) : null}
      {active && editing ? (
        <div className="aw-agentcard__edit">
          <AgentEditForm
            connection={connection}
            onClose={() => setEditing(false)}
            onSecret={setSecret}
            projectId={projectId}
          />
          <WorkOsAgentSettings connection={connection} />
        </div>
      ) : null}
      {connection.status !== 'removed' && (
        <footer className="aw-card__actions aw-agentcard__actions">
          {active ? (
            editing ? null : (
              <Button icon="pencil" onClick={() => setEditing(true)} size="sm" variant="ghost">
                Edit
              </Button>
            )
          ) : (
            <Button disabled={disabled} icon="circle-check" onClick={() => onEnable(true)} size="sm">
              Turn back on
            </Button>
          )}
          <span className="aw-agentcard__spacer" />
          <Menu
            items={menu}
            label={`${connection.displayName} actions`}
            placement="bottom-end"
            trigger={<IconButton icon="more" label={`${connection.displayName} actions`} size="sm" />}
          />
        </footer>
      )}
    </article>
  )
}

/** Access, what it can do and where wakeups go, saved together. */
function AgentEditForm({
  connection,
  onClose,
  onSecret,
  projectId,
}: {
  connection: ExternalAgentConnection
  onClose: () => void
  onSecret: (secret: string | null) => void
  projectId: string
}) {
  const queryClient = useQueryClient()
  const [role, setRole] = useState<ExternalAgentRole>(connection.role)
  const [scopes, setScopes] = useState<ExternalAgentScope[]>(connection.scopes)
  const [url, setUrl] = useState(connection.webhookUrl ?? '')
  const [test, setTest] = useState<{ tone: 'success' | 'danger'; text: string } | null>(null)
  const savedUrl = connection.webhookUrl ?? ''
  const roleChanged = role !== connection.role
  const scopesChanged = scopes.length !== connection.scopes.length || scopes.some((scope) => !connection.scopes.includes(scope))
  const urlChanged = url.trim() !== savedUrl
  const dirty = roleChanged || scopesChanged || urlChanged
  const refresh = () => queryClient.invalidateQueries({ queryKey: queryKeys.externalAgents(projectId) })

  const save = useMutation({
    mutationFn: async (input: { resaveWebhook?: boolean } = {}) => {
      let secret: string | null = null
      if (!input.resaveWebhook && (roleChanged || scopesChanged)) {
        await updateExternalAgent(projectId, connection.id, {
          expectedVersion: connection.version,
          ...(roleChanged ? { role } : {}),
          ...(scopesChanged ? { scopes } : {}),
        })
      }
      if (input.resaveWebhook || urlChanged) {
        const result = await setExternalAgentWebhook(projectId, connection.id, input.resaveWebhook ? savedUrl : url.trim() || null)
        secret = result.webhookSecret
      }
      return secret
    },
    onSuccess: (secret, input) => {
      showToast({ tone: 'success', title: input?.resaveWebhook ? 'Wakeups updated' : 'Changes saved' })
      if (secret) onSecret(secret)
      if (!input?.resaveWebhook) onClose()
    },
    onSettled: refresh,
  })
  const sendTest = useMutation({
    mutationFn: () => testExternalAgentWebhook(projectId, connection.id),
    onMutate: () => setTest(null),
    onSuccess: (result) =>
      setTest(
        result.ok
          ? { tone: 'success', text: `The runner answered ${result.status ?? 'OK'} in ${result.elapsedMs} ms.` }
          : { tone: 'danger', text: `The test wakeup didn’t get through: ${result.error ?? 'no answer'}.` },
      ),
    onError: (reason) => setTest({ tone: 'danger', text: `The test wakeup wasn’t sent. ${describeError(reason).message}` }),
  })
  const changeRole = (next: ExternalAgentRole) => {
    setRole(next)
    setScopes(next === 'viewer' ? viewerScopes : next === connection.role ? connection.scopes : memberScopes)
  }
  const submit = (event: FormEvent<HTMLFormElement>) => {
    event.preventDefault()
    if (dirty) save.mutate({})
  }
  const busy = save.isPending
  return (
    <form className="aw-agentcard__form" onSubmit={submit}>
      <SelectField
        disabled={busy}
        id={`agent-role-${connection.id}`}
        label="Access"
        onChange={(event) => changeRole(event.target.value as ExternalAgentRole)}
        value={role}
      >
        <RoleOptions />
      </SelectField>
      <ScopeFields disabled={busy} onChange={setScopes} role={role} scopes={scopes} />
      <div className="aw-webhook">
        {connection.webhookUrl && connection.wakeupFormat === 'inbox' ? (
          <Banner
            action={
              <Button busy={busy} onClick={() => save.mutate({ resaveWebhook: true })} size="sm">
                Update wakeups
              </Button>
            }
            tone="warning"
          >
            This webhook gets the old “check your inbox” wakeups, which stop working in a later release. Update it for wakeups
            that say what happened (a new request, a follow-up, a stop), then give the runner the new secret.
          </Banner>
        ) : null}
        <TextField
          disabled={busy}
          hint={
            connection.lastWakeupAt ? (
              <>
                <span title={formatFullDate(connection.lastWakeupAt)}>Last wakeup {formatRelative(connection.lastWakeupAt)}</span>
                {connection.lastWakeupError ? `, which failed: ${connection.lastWakeupError}` : ''}. Leave it empty to stop wakeups.
              </>
            ) : (
              'We call this when someone asks the agent for a reply, follows up, or stops it. Leave it empty for agents that check in on a schedule.'
            )
          }
          id={`webhook-${connection.id}`}
          label="Wakeup webhook (optional)"
          onChange={(event) => setUrl(event.target.value)}
          placeholder="https://…"
          spellCheck={false}
          type="url"
          value={url}
        />
        {test ? <Banner tone={test.tone}>{test.text}</Banner> : null}
      </div>
      {save.isError ? <Banner tone="danger">{`Your changes weren’t saved. ${describeError(save.error).message}`}</Banner> : null}
      <div className="aw-card__actions">
        <Button busy={busy} disabled={!dirty} type="submit" variant="primary">
          Save changes
        </Button>
        <Button disabled={busy} onClick={onClose}>
          Cancel
        </Button>
        {connection.webhookUrl ? (
          <Button busy={sendTest.isPending} disabled={busy || urlChanged} onClick={() => sendTest.mutate()} variant="ghost">
            Send a test wakeup
          </Button>
        ) : null}
      </div>
    </form>
  )
}

function useCopy() {
  const [copied, setCopied] = useState<'yes' | 'failed' | null>(null)
  const copy = (value: string) => {
    if (!navigator.clipboard) {
      setCopied('failed')
      return
    }
    navigator.clipboard.writeText(value).then(
      () => setCopied('yes'),
      () => setCopied('failed'),
    )
  }
  return [copied, copy] as const
}

function OneTimeToken({
  agentName,
  credential,
  onDismiss,
  setup,
}: {
  agentName: string
  credential: OneTimeCredential
  onDismiss: () => void
  setup?: { handle: string; runner: RunnerPresetId; webhookSecret: string | null }
}) {
  const [copied, copyToken] = useCopy()
  const [settingsCopied, copySettings] = useCopy()
  const settings = setup
    ? runnerSettings({
        origin: window.location.origin,
        token: credential.token,
        webhookSecret: setup.webhookSecret,
        handle: setup.handle,
        runner: setup.runner,
      })
    : null
  const runner = setup ? setup.runner !== 'agent' : false
  return (
    <aside aria-labelledby="external-bearer-ready-heading" className="aw-card aw-token">
      <div className="aw-token__head">
        <Icon name="shield-check" size={18} />
        <h3 id="external-bearer-ready-heading">Token ready for {agentName}</h3>
      </div>
      <p>Copy it into the agent’s secret file now. It can’t be shown again after you close this.</p>
      <code className="aw-token__value">{credential.token}</code>
      <p className="aw-token__meta" title={formatFullDate(credential.expiresAt)}>
        Expires {formatShortDate(credential.expiresAt)}
      </p>
      {settings && runner ? (
        <>
          <p>Your runner’s settings, with the token and signing secret in them. Keep them somewhere private.</p>
          <pre aria-label="Runner settings" className="aw-token__settings">
            {settings}
          </pre>
          <p className="aw-token__help">
            The runner checks the signature on each wakeup, claims the request, posts progress, answers in the thread and stops
            when someone stops it.{' '}
            <a href="/agent-runner.md" rel="noreferrer" target="_blank">
              How a runner works
            </a>
          </p>
        </>
      ) : null}
      <p className="aw-token__help" hidden={runner}>
        Point the agent’s MCP client at <code>{new URL(apiUrl('/api/v1/mcp'), window.location.href).href}</code> with this token, or send it the{' '}
        <a href="/connect.md" rel="noreferrer" target="_blank">
          setup guide
        </a>{' '}
        for step-by-step instructions.
      </p>
      <div className="aw-card__actions">
        <Button icon={copied === 'yes' ? 'check' : 'copy'} onClick={() => copyToken(credential.token)} variant="primary">
          {copied === 'yes' ? 'Copied' : 'Copy token'}
        </Button>
        {settings && runner ? (
          <Button icon={settingsCopied === 'yes' ? 'check' : 'copy'} onClick={() => copySettings(settings)}>
            {settingsCopied === 'yes' ? 'Copied settings' : 'Copy settings'}
          </Button>
        ) : null}
        <Button onClick={onDismiss}>Done</Button>
      </div>
      {copied === 'failed' || settingsCopied === 'failed' ? (
        <p className="aw-quiet" role="status">
          Couldn’t copy. Select the text above and copy it yourself.
        </p>
      ) : null}
    </aside>
  )
}
