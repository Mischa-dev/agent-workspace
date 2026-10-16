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
  MessageSkeleton,
  Section,
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
  updateExternalAgent,
  type ExternalAgentConnection,
  type ExternalAgentMutation,
  type ExternalAgentRole,
  type ExternalAgentScope,
  type OneTimeCredential,
} from './api'

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
  const [oneTimeCredential, setOneTimeCredential] = useState<{ agentName: string; credential: OneTimeCredential } | null>(null)
  const [error, setError] = useState<string | null>(null)
  const errorRef = useRef<HTMLDivElement>(null)
  useEffect(() => {
    if (error) errorRef.current?.focus()
  }, [error])

  const refresh = async () => {
    await queryClient.invalidateQueries({ queryKey: queryKeys.externalAgents(projectId) })
  }
  const handleSuccess = async (result: ExternalAgentMutation) => {
    if (result.oneTimeCredential) {
      setOneTimeCredential({ agentName: result.connection.displayName, credential: result.oneTimeCredential })
    }
    await refresh()
  }
  const handleError = (reason: unknown) => {
    setError(isApiError(reason) ? reason.message : 'That change to the agent didn’t go through. Try again.')
  }
  const createMutation = useMutation({
    mutationFn: (input: Parameters<typeof createExternalAgent>[1]) => createExternalAgent(projectId, input),
    onSuccess: handleSuccess,
    onError: handleError,
  })
  const updateMutation = useMutation({
    mutationFn: ({ connection, input }: { connection: ExternalAgentConnection; input: Parameters<typeof updateExternalAgent>[2] }) =>
      updateExternalAgent(projectId, connection.id, input),
    onSuccess: handleSuccess,
    onError: handleError,
  })
  const rotateMutation = useMutation({
    mutationFn: (connection: ExternalAgentConnection) => rotateExternalAgentCredential(projectId, connection.id, connection.version),
    onSuccess: handleSuccess,
    onError: handleError,
  })
  const removeMutation = useMutation({
    mutationFn: (connection: ExternalAgentConnection) => removeExternalAgent(projectId, connection.id, connection.version),
    onSuccess: handleSuccess,
    onError: handleError,
  })
  const pending = createMutation.isPending || updateMutation.isPending || rotateMutation.isPending || removeMutation.isPending

  if (connections.isPending) return <MessageSkeleton label="Loading agents" rows={2} />
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
      <Section description="You’ll get a token for its MCP client once. It can’t be shown again." title="Connect an agent">
        <CreateAgentForm
          disabled={pending}
          onSubmit={(input) => {
            setError(null)
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
  onSubmit: (input: Parameters<typeof createExternalAgent>[1]) => void
}) {
  const [connectionName, setConnectionName] = useState('')
  const [displayName, setDisplayName] = useState('')
  const [handle, setHandle] = useState('')
  const [role, setRole] = useState<ExternalAgentRole>('member')
  const [scopes, setScopes] = useState<ExternalAgentScope[]>(memberScopes)
  const submit = (event: FormEvent<HTMLFormElement>) => {
    event.preventDefault()
    onSubmit({ connectionName, displayName, handle, role, scopes })
  }
  const changeRole = (nextRole: ExternalAgentRole) => {
    setRole(nextRole)
    setScopes(nextRole === 'viewer' ? viewerScopes : memberScopes)
  }
  return (
    <form className="aw-card aw-agentform" onSubmit={submit}>
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
      </div>
      <ScopeFields disabled={disabled} onChange={setScopes} role={role} scopes={scopes} />
      <div className="aw-form__actions">
        <Button busy={disabled} icon="bot" type="submit" variant="primary">
          Connect agent
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
}: {
  agentName: string
  credential: OneTimeCredential
  onDismiss: () => void
}) {
  const [copied, setCopied] = useState(false)
  return (
    <aside aria-labelledby="external-bearer-ready-heading" className="aw-card aw-token">
      <div className="aw-token__head">
        <Icon name="shield-check" size={18} />
        <h3 id="external-bearer-ready-heading">Token ready for {agentName}</h3>
      </div>
      <p>Copy it into the agent’s secret file now. It can’t be shown again after you close this.</p>
      <code className="aw-token__value">{credential.token}</code>
      <p className="aw-token__meta">Expires {formatWhen(credential.expiresAt) ?? credential.expiresAt}</p>
      <p className="aw-token__help">
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
  const save = (next: string | null) => {
    setBusy(true)
    setError(null)
    void setExternalAgentWebhook(connection.projectId, connection.id, next)
      .then(async (result) => {
        setSecret(result.webhookSecret)
        await queryClient.invalidateQueries({ queryKey: queryKeys.externalAgents(connection.projectId) })
      })
      .catch((reason: unknown) => setError(reason instanceof Error ? reason.message : 'The webhook wasn’t saved.'))
      .finally(() => setBusy(false))
  }
  const lastWakeup = formatWhen(connection.lastWakeupAt)
  return (
    <div className="aw-webhook">
      <TextField
        disabled={disabled || busy}
        error={error ?? undefined}
        hint={
          lastWakeup
            ? `Last wakeup ${lastWakeup}${connection.lastWakeupError ? `, which failed: ${connection.lastWakeupError}` : ''}.`
            : 'We call this when someone asks the agent for a reply. Leave it empty for agents that check in on a schedule.'
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
          <Button disabled={disabled || busy} onClick={() => save(null)} size="sm" variant="ghost">
            Stop wakeups
          </Button>
        ) : null}
      </div>
      {secret ? (
        <p className="aw-token__meta">
          Signing secret, shown once: <code>{secret}</code>
        </p>
      ) : null}
    </div>
  )
}
