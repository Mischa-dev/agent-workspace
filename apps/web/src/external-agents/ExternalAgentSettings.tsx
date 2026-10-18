import { useEffect, useRef, useState, type FormEvent } from 'react'
import { useMutation, useQuery, useQueryClient } from '@tanstack/react-query'

import { WorkOsAgentSettings } from './WorkOsAgentSettings'
import { isApiError } from '../api/errors'
import { queryKeys } from '../api/queryKeys'
import {
  ActorTag,
  Avatar,
  Banner,
  Button,
  ErrorState,
  Icon,
  ListSkeleton,
  Section,
  SegmentedControl,
  SelectField,
  StatusChip,
  TextField,
  formatDateTime,
} from '../ui'
import {
  createExternalAgent,
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

function formatWhen(iso: string | null | undefined) {
  if (!iso) return null
  return formatDateTime(iso) || iso
}

export function ExternalAgentSettings({ projectId }: { projectId: string }) {
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
  const [creatingAs, setCreatingAs] = useState<RunnerPresetId>('agent')
  const [error, setError] = useState<string | null>(null)
  const errorRef = useRef<HTMLDivElement>(null)
  useEffect(() => {
    if (error) errorRef.current?.focus()
  }, [error])

  const refresh = async () => {
    await queryClient.invalidateQueries({ queryKey: queryKeys.externalAgents(projectId) })
  }
  const handleSuccess = async (result: ExternalAgentMutation, created = false) => {
    if (result.oneTimeCredential) {
      setOneTimeCredential({
        agentName: result.connection.displayName,
        credential: result.oneTimeCredential,
        setup: created
          ? { handle: result.connection.handle, runner: creatingAs, webhookSecret: result.webhookSecret ?? null }
          : undefined,
      })
    }
    await refresh()
  }
  const handleError = (reason: unknown) => {
    setError(isApiError(reason) ? reason.message : 'That change to the agent didn’t go through. Try again.')
  }
  const createMutation = useMutation({
    mutationFn: (input: Parameters<typeof createExternalAgent>[1]) => createExternalAgent(projectId, input),
    onSuccess: (result) => handleSuccess(result, true),
    onError: handleError,
  })
  const updateMutation = useMutation({
    mutationFn: ({ connection, input }: { connection: ExternalAgentConnection; input: Parameters<typeof updateExternalAgent>[2] }) =>
      updateExternalAgent(projectId, connection.id, input),
    onSuccess: (result) => handleSuccess(result),
    onError: handleError,
  })
  const rotateMutation = useMutation({
    mutationFn: (connection: ExternalAgentConnection) => rotateExternalAgentCredential(projectId, connection.id, connection.version),
    onSuccess: (result) => handleSuccess(result),
    onError: handleError,
  })
  const removeMutation = useMutation({
    mutationFn: (connection: ExternalAgentConnection) => removeExternalAgent(projectId, connection.id, connection.version),
    onSuccess: (result) => handleSuccess(result),
    onError: handleError,
  })
  const pending = createMutation.isPending || updateMutation.isPending || rotateMutation.isPending || removeMutation.isPending

  if (connections.isPending) return <ListSkeleton label="Loading agents" rows={2} avatars />
  if (connections.isError) {
    return (
      <ErrorState action={<Button onClick={() => void connections.refetch()}>Try again</Button>} title="Agents didn’t load">
        Check your connection and try again.
      </ErrorState>
    )
  }

  return (
    <div className="aw-agents">
      {error && (
        <Banner className="aw-agents__error" ref={errorRef} tabIndex={-1} tone="danger">
          {error}
        </Banner>
      )}
      {oneTimeCredential && (
        <OneTimeToken
          agentName={oneTimeCredential.agentName}
          credential={oneTimeCredential.credential}
          onDismiss={() => setOneTimeCredential(null)}
          setup={oneTimeCredential.setup}
        />
      )}
      <Section description="The agents that can read and post here." title="Connected agents">
        {!connections.data.connections.length && <p className="aw-agents__empty">No agents connected yet.</p>}
        <div className="aw-agents__list">
          {connections.data.connections.map((connection) => (
            <AgentCard
              connection={connection}
              disabled={pending}
              key={connection.id}
              onRemove={() => {
                setError(null)
                removeMutation.mutate(connection)
              }}
              onRotate={() => {
                setError(null)
                rotateMutation.mutate(connection)
              }}
              onUpdate={(input) => {
                setError(null)
                updateMutation.mutate({ connection, input })
              }}
            />
          ))}
        </div>
      </Section>
      <Section
        description="Pick a runner to tag it in conversations (@claude, @codex), or connect any other agent. You’ll get its token once; it can’t be shown again."
        title="Connect an agent"
      >
        <CreateAgentForm
          disabled={pending}
          onSubmit={(preset, input) => {
            setError(null)
            setCreatingAs(preset)
            createMutation.mutate(input)
          }}
        />
      </Section>
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
            label="Role"
            onChange={(event) => changeRole(event.target.value as ExternalAgentRole)}
            value={role}
          >
            <option value="member">Member: reads and posts</option>
            <option value="viewer">Viewer: reads only</option>
          </SelectField>
        )}
      </div>
      {runner ? null : <ScopeFields disabled={disabled} onChange={setScopes} role={role} scopes={scopes} />}
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
  onRemove,
  onRotate,
  onUpdate,
}: {
  connection: ExternalAgentConnection
  disabled: boolean
  onRemove: () => void
  onRotate: () => void
  onUpdate: (input: Parameters<typeof updateExternalAgent>[2]) => void
}) {
  const active = connection.status === 'active'
  const [removeArmed, setRemoveArmed] = useState(false)
  const [editing, setEditing] = useState(false)
  const lastUsed = formatWhen(connection.lastUsedAt)
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
            @{connection.handle} · {connection.role === 'viewer' ? 'Viewer' : 'Member'} · {lastUsed ? `last active ${lastUsed}` : 'not used yet'}
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
      {active && editing ? (
        <div className="aw-agentcard__edit">
          <SelectField
            aria-label={`Role for ${connection.displayName}`}
            disabled={disabled}
            label="Role"
            onChange={(event) => {
              const role = event.target.value as ExternalAgentRole
              onUpdate({ expectedVersion: connection.version, role, scopes: role === 'viewer' ? viewerScopes : connection.scopes })
            }}
            value={connection.role}
          >
            <option value="member">Member: reads and posts</option>
            <option value="viewer">Viewer: reads only</option>
          </SelectField>
          <ScopeFields
            disabled={disabled}
            onChange={(scopes) => onUpdate({ expectedVersion: connection.version, scopes })}
            role={connection.role}
            scopes={connection.scopes}
          />
          <WebhookField connection={connection} disabled={disabled} />
          <WorkOsAgentSettings connection={connection} />
        </div>
      ) : null}
      {connection.status !== 'removed' && (
        <footer className="aw-card__actions">
          {active ? (
            <>
              <Button icon="settings" onClick={() => setEditing(!editing)} size="sm" variant="ghost">
                {editing ? 'Done editing' : 'Edit'}
              </Button>
              <Button disabled={disabled} icon="retry" onClick={onRotate} size="sm" variant="ghost">
                New token
              </Button>
              <Button
                disabled={disabled}
                icon="circle-minus"
                onClick={() => onUpdate({ expectedVersion: connection.version, enabled: false })}
                size="sm"
                variant="ghost"
              >
                Disable
              </Button>
            </>
          ) : (
            <Button
              disabled={disabled}
              icon="circle-check"
              onClick={() => onUpdate({ expectedVersion: connection.version, enabled: true })}
              size="sm"
            >
              Re-enable
            </Button>
          )}
          <span className="aw-agentcard__spacer" />
          {removeArmed ? (
            <>
              <Button disabled={disabled} onClick={() => setRemoveArmed(false)} size="sm">
                Cancel
              </Button>
              <Button disabled={disabled} icon="trash" onClick={onRemove} size="sm" variant="danger">
                Confirm removal
              </Button>
            </>
          ) : (
            <Button disabled={disabled} icon="trash" onClick={() => setRemoveArmed(true)} size="sm" variant="ghost">
              Remove…
            </Button>
          )}
        </footer>
      )}
    </article>
  )
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
  const [copied, setCopied] = useState(false)
  const [settingsCopied, setSettingsCopied] = useState(false)
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
      <p className="aw-token__meta">Expires {formatWhen(credential.expiresAt) ?? credential.expiresAt}</p>
      {setup?.webhookSecret ? (
        <p className="aw-token__meta">
          Webhook signing secret: <code>{setup.webhookSecret}</code>
        </p>
      ) : null}
      {settings && runner ? (
        <>
          <p>Your runner’s settings, with the token and secret in them. Keep them somewhere private.</p>
          <pre aria-label="Runner settings" className="aw-token__settings">
            {settings}
          </pre>
          <p className="aw-token__help">
            The runner checks the signature on each wakeup, claims the request with <code>reply_claim</code>, reports
            with <code>reply_progress</code>, answers with <code>reply_send</code>, and stops when it hears{' '}
            <code>reply.cancelled</code>. The whole contract is at{' '}
            <a href="/agent-runner.md" rel="noreferrer" target="_blank">{`${window.location.origin}/agent-runner.md`}</a>.
          </p>
        </>
      ) : null}
      <p className="aw-token__help" hidden={runner}>
        Point the agent’s MCP client at <code>{`${window.location.origin}/api/v1/mcp`}</code> with this token. It checks{' '}
        <code>inbox_list</code>, then answers with <code>reply_claim</code> and <code>reply_send</code>, or <code>reply_pass</code>.
        Send the agent <a href="/connect.md" rel="noreferrer" target="_blank">{`${window.location.origin}/connect.md`}</a> for
        step-by-step setup.
      </p>
      <div className="aw-card__actions">
        <Button
          icon={copied ? 'check' : 'copy'}
          onClick={() => {
            void navigator.clipboard.writeText(credential.token).then(() => setCopied(true))
          }}
          variant="primary"
        >
          {copied ? 'Copied' : 'Copy token'}
        </Button>
        {settings && runner ? (
          <Button
            icon={settingsCopied ? 'check' : 'copy'}
            onClick={() => {
              void navigator.clipboard.writeText(settings).then(() => setSettingsCopied(true))
            }}
          >
            {settingsCopied ? 'Copied settings' : 'Copy settings'}
          </Button>
        ) : null}
        <Button onClick={onDismiss}>Done</Button>
      </div>
    </aside>
  )
}

/** Optional push wakeups. Agents that can't receive webhooks check their inbox on a schedule instead. */
function WebhookField({ connection, disabled }: { connection: ExternalAgentConnection; disabled: boolean }) {
  const queryClient = useQueryClient()
  const [url, setUrl] = useState(connection.webhookUrl ?? '')
  const [secret, setSecret] = useState<string | null>(null)
  const [error, setError] = useState<string | null>(null)
  const [busy, setBusy] = useState(false)
  const [test, setTest] = useState<{ tone: 'success' | 'danger'; text: string } | null>(null)
  const save = (next: string | null) => {
    setBusy(true)
    setError(null)
    setTest(null)
    void setExternalAgentWebhook(connection.projectId, connection.id, next)
      .then(async (result) => {
        setSecret(result.webhookSecret)
        await queryClient.invalidateQueries({ queryKey: queryKeys.externalAgents(connection.projectId) })
      })
      .catch((reason: unknown) => setError(reason instanceof Error ? reason.message : 'The webhook wasn’t saved.'))
      .finally(() => setBusy(false))
  }
  const sendTest = () => {
    setBusy(true)
    setTest(null)
    void testExternalAgentWebhook(connection.projectId, connection.id)
      .then((result) =>
        setTest(
          result.ok
            ? { tone: 'success', text: `The runner answered ${result.status ?? 'OK'} in ${result.elapsedMs} ms.` }
            : { tone: 'danger', text: `The test wakeup didn’t get through: ${result.error ?? 'no answer'}.` },
        ),
      )
      .catch((reason: unknown) =>
        setTest({ tone: 'danger', text: reason instanceof Error ? reason.message : 'The test wakeup wasn’t sent.' }),
      )
      .finally(() => setBusy(false))
  }
  const lastWakeup = formatWhen(connection.lastWakeupAt)
  return (
    <div className="aw-webhook">
      {connection.webhookUrl && connection.wakeupFormat === 'inbox' ? (
        <Banner tone="warning">
          This webhook gets the old “check your inbox” wakeups, which stop working in a later release. Save it again for
          wakeups that say what happened (a new request, a follow-up, a stop), then give the runner the new secret.
        </Banner>
      ) : null}
      <TextField
        disabled={disabled || busy}
        error={error ?? undefined}
        hint={
          lastWakeup
            ? `Last wakeup ${lastWakeup}${connection.lastWakeupError ? `, which failed: ${connection.lastWakeupError}` : ''}.`
            : 'We call this when someone asks the agent for a reply, follows up, or stops it. Leave it empty for agents that check in on a schedule.'
        }
        id={`webhook-${connection.id}`}
        label="Wakeup webhook (optional)"
        onChange={(event) => setUrl(event.target.value)}
        placeholder="https://…"
        type="url"
        value={url}
      />
      <div className="aw-card__actions">
        <Button disabled={disabled || busy || !url.trim()} onClick={() => save(url.trim())} size="sm">
          Save webhook
        </Button>
        {connection.webhookUrl ? (
          <Button disabled={disabled || busy} onClick={sendTest} size="sm" variant="secondary">
            Send a test wakeup
          </Button>
        ) : null}
        {connection.webhookUrl ? (
          <Button disabled={disabled || busy} onClick={() => save(null)} size="sm" variant="ghost">
            Stop wakeups
          </Button>
        ) : null}
      </div>
      {test ? <Banner tone={test.tone}>{test.text}</Banner> : null}
      {secret ? (
        <p className="aw-token__meta">
          Signing secret, shown once: <code>{secret}</code>
        </p>
      ) : null}
    </div>
  )
}
