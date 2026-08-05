import { FormEvent, useEffect, useRef, useState } from 'react'
import { useMutation, useQuery, useQueryClient } from '@tanstack/react-query'

import { isApiError } from '../api/errors'
import { queryKeys } from '../api/queryKeys'
import {
  createExternalAgent,
  listExternalAgents,
  removeExternalAgent,
  rotateExternalAgentCredential,
  updateExternalAgent,
  type ExternalAgentConnection,
  type ExternalAgentMutation,
  type ExternalAgentRole,
  type ExternalAgentScope,
  type OneTimeCredential,
} from './api'

const memberScopes: ExternalAgentScope[] = [
  'read',
  'write',
  'events',
  'approval_respond',
  'worker_request',
]
const viewerScopes: ExternalAgentScope[] = ['read', 'events']

export function ExternalAgentSettings({ projectId }: { projectId: string }) {
  const queryClient = useQueryClient()
  const connections = useQuery({
    queryKey: queryKeys.externalAgents(projectId),
    queryFn: () => listExternalAgents(projectId),
  })
  const [oneTimeCredential, setOneTimeCredential] = useState<{
    agentName: string
    credential: OneTimeCredential
  } | null>(null)
  const [error, setError] = useState<string | null>(null)
  const errorRef = useRef<HTMLDivElement>(null)
  useEffect(() => {
    if (error) errorRef.current?.focus()
  }, [error])

  const refresh = async () => {
    await queryClient.invalidateQueries({
      queryKey: queryKeys.externalAgents(projectId),
    })
  }
  const handleSuccess = async (result: ExternalAgentMutation) => {
    if (result.oneTimeCredential) {
      setOneTimeCredential({
        agentName: result.connection.displayName,
        credential: result.oneTimeCredential,
      })
    }
    await refresh()
  }
  const handleError = (reason: unknown) => {
    setError(
      isApiError(reason)
        ? reason.message
        : 'The external agent change could not be completed.',
    )
  }
  const createMutation = useMutation({
    mutationFn: (input: Parameters<typeof createExternalAgent>[1]) =>
      createExternalAgent(projectId, input),
    onSuccess: handleSuccess,
    onError: handleError,
  })
  const updateMutation = useMutation({
    mutationFn: ({
      connection,
      input,
    }: {
      connection: ExternalAgentConnection
      input: Parameters<typeof updateExternalAgent>[2]
    }) => updateExternalAgent(projectId, connection.id, input),
    onSuccess: handleSuccess,
    onError: handleError,
  })
  const rotateMutation = useMutation({
    mutationFn: (connection: ExternalAgentConnection) =>
      rotateExternalAgentCredential(
        projectId,
        connection.id,
        connection.version,
      ),
    onSuccess: handleSuccess,
    onError: handleError,
  })
  const removeMutation = useMutation({
    mutationFn: (connection: ExternalAgentConnection) =>
      removeExternalAgent(projectId, connection.id, connection.version),
    onSuccess: handleSuccess,
    onError: handleError,
  })
  const pending =
    createMutation.isPending ||
    updateMutation.isPending ||
    rotateMutation.isPending ||
    removeMutation.isPending

  if (connections.isPending) return <p aria-live="polite">Loading external agents…</p>
  if (connections.isError) {
    return (
      <div className="error-state" role="alert">
        <p>External agent settings could not be loaded.</p>
        <button onClick={() => void connections.refetch()} type="button">
          Try again
        </button>
      </div>
    )
  }

  return (
    <div className="external-agent-settings">
      {error && (
        <div className="form-error-summary" ref={errorRef} role="alert" tabIndex={-1}>
          {error}
        </div>
      )}
      {oneTimeCredential && (
        <OneTimeBearer
          agentName={oneTimeCredential.agentName}
          credential={oneTimeCredential.credential}
          onDismiss={() => setOneTimeCredential(null)}
        />
      )}
      <CreateExternalAgentForm
        disabled={pending}
        onSubmit={(input) => {
          setError(null)
          createMutation.mutate(input)
        }}
      />
      <h3>Connected teammates</h3>
      {!connections.data.connections.length && (
        <p>No external agent connections have been created.</p>
      )}
      <div className="external-agent-list">
        {connections.data.connections.map((connection) => (
          <ExternalAgentRow
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
    </div>
  )
}

function CreateExternalAgentForm({
  disabled,
  onSubmit,
}: {
  disabled: boolean
  onSubmit: (input: Parameters<typeof createExternalAgent>[1]) => void
}) {
  const [connectionName, setConnectionName] = useState('Hermes')
  const [displayName, setDisplayName] = useState('Hermes teammate')
  const [handle, setHandle] = useState('hermes')
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
    <form className="external-agent-form" onSubmit={submit}>
      <div className="field">
        <label htmlFor="external-agent-connection-name">Connection name</label>
        <input
          disabled={disabled}
          id="external-agent-connection-name"
          maxLength={80}
          onChange={(event) => setConnectionName(event.target.value)}
          required
          value={connectionName}
        />
      </div>
      <div className="field">
        <label htmlFor="external-agent-display-name">Display name</label>
        <input
          disabled={disabled}
          id="external-agent-display-name"
          maxLength={80}
          onChange={(event) => setDisplayName(event.target.value)}
          required
          value={displayName}
        />
      </div>
      <div className="field">
        <label htmlFor="external-agent-handle">Handle</label>
        <input
          disabled={disabled}
          id="external-agent-handle"
          maxLength={32}
          onChange={(event) => setHandle(event.target.value)}
          required
          spellCheck={false}
          value={handle}
        />
      </div>
      <div className="field">
        <label htmlFor="external-agent-role">Project role</label>
        <select
          disabled={disabled}
          id="external-agent-role"
          onChange={(event) => changeRole(event.target.value as ExternalAgentRole)}
          value={role}
        >
          <option value="member">Member</option>
          <option value="viewer">Viewer</option>
        </select>
      </div>
      <ScopeFields
        disabled={disabled}
        onChange={setScopes}
        role={role}
        scopes={scopes}
      />
      <button className="primary-button" disabled={disabled} type="submit">
        {disabled ? 'Connecting…' : 'Connect external agent'}
      </button>
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
    <fieldset className="external-agent-scopes">
      <legend>Connection permissions</legend>
      {allowed.map((scope) => (
        <label key={scope}>
          <input
            checked={scopes.includes(scope)}
            disabled={disabled || scope === 'read'}
            onChange={(event) =>
              onChange(
                event.target.checked
                  ? [...scopes, scope]
                  : scopes.filter((candidate) => candidate !== scope),
              )
            }
            type="checkbox"
          />
          {scope.replace('_', ' ')}
        </label>
      ))}
    </fieldset>
  )
}

function ExternalAgentRow({
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
  return (
    <article className="external-agent-row">
      <div>
        <strong>{connection.displayName}</strong>
        <span>@{connection.handle}</span>
        <small>
          {connection.status} · Hermes {connection.runtimeVersion} ·{' '}
          {connection.lastUsedAt ? `last active ${connection.lastUsedAt}` : 'not used yet'}
        </small>
        <small>Permissions: {connection.scopes.join(', ')}</small>
        {active && (
          <ScopeFields
            disabled={disabled}
            onChange={(scopes) =>
              onUpdate({ expectedVersion: connection.version, scopes })
            }
            role={connection.role}
            scopes={connection.scopes}
          />
        )}
      </div>
      {connection.status !== 'removed' && (
        <div className="row-actions">
          <select
            aria-label={`Role for ${connection.displayName}`}
            disabled={disabled || !active}
            onChange={(event) => {
              const role = event.target.value as ExternalAgentRole
              onUpdate({
                expectedVersion: connection.version,
                role,
                scopes: role === 'viewer' ? viewerScopes : connection.scopes,
              })
            }}
            value={connection.role}
          >
            <option value="member">Member</option>
            <option value="viewer">Viewer</option>
          </select>
          {active ? (
            <>
              <button disabled={disabled} onClick={onRotate} type="button">
                Rotate bearer
              </button>
              <button
                disabled={disabled}
                onClick={() =>
                  onUpdate({ expectedVersion: connection.version, enabled: false })
                }
                type="button"
              >
                Disable
              </button>
            </>
          ) : (
            <button
              disabled={disabled}
              onClick={() =>
                onUpdate({ expectedVersion: connection.version, enabled: true })
              }
              type="button"
            >
              Re-enable
            </button>
          )}
          {removeArmed ? (
            <>
              <button
                className="danger-button"
                disabled={disabled}
                onClick={onRemove}
                type="button"
              >
                Confirm removal
              </button>
              <button
                disabled={disabled}
                onClick={() => setRemoveArmed(false)}
                type="button"
              >
                Cancel
              </button>
            </>
          ) : (
            <button
              className="danger-button"
              disabled={disabled}
              onClick={() => setRemoveArmed(true)}
              type="button"
            >
              Remove…
            </button>
          )}
        </div>
      )}
    </article>
  )
}

function OneTimeBearer({
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
    <aside aria-labelledby="external-bearer-ready-heading" className="one-shot-secret">
      <h3 id="external-bearer-ready-heading">Bearer ready for {agentName}</h3>
      <p>
        Copy it now into the external runtime’s owner-only secret file. It cannot
        be recovered after this panel closes.
      </p>
      <code>{credential.token}</code>
      <small>Expires {credential.expiresAt}</small>
      <div className="row-actions">
        <button
          onClick={() => {
            void navigator.clipboard
              .writeText(credential.token)
              .then(() => setCopied(true))
          }}
          type="button"
        >
          {copied ? 'Copied' : 'Copy bearer'}
        </button>
        <button onClick={onDismiss} type="button">
          Done
        </button>
      </div>
    </aside>
  )
}
