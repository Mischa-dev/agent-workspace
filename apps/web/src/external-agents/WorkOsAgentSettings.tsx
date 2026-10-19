import { useState } from 'react'
import { useMutation, useQuery, useQueryClient } from '@tanstack/react-query'

import { requestJson } from '../api/client'
import { Banner, Button, ListSkeleton, Row, RowList, TextField, describeError, describeLoadError } from '../ui'
import type { ExternalAgentConnection } from './api'

type Binding = { id: string; kind: 'blueprint' | 'registration'; subjectId: string; revokedAt: string | null }

/** Single sign-on for one agent: short-lived tokens from the organization’s sign-in in place of its connection token. */
export function WorkOsAgentSettings({ connection }: { connection: ExternalAgentConnection }) {
  const path = `/api/v1/projects/${encodeURIComponent(connection.projectId)}/external-agents/${encodeURIComponent(connection.id)}/workos`
  const client = useQueryClient()
  const [registrationId, setRegistrationId] = useState('')
  const [error, setError] = useState<string | null>(null)
  const queryKey = ['workos-agent', connection.id]
  const bindings = useQuery({
    queryKey,
    queryFn: () => requestJson<{ enabled: boolean; registrationEnabled: boolean; bindings: Binding[] }>(path),
  })
  const update = useMutation({
    mutationFn: (input: { registrationId?: string; revoke?: string }) =>
      input.revoke
        ? requestJson(`${path}/${encodeURIComponent(input.revoke)}`, { method: 'DELETE' })
        : requestJson(path, { method: 'POST', json: input }),
    onSuccess: async () => {
      setError(null)
      setRegistrationId('')
      await client.invalidateQueries({ queryKey })
    },
    onError: (reason) => setError(`Single sign-on for this agent wasn’t changed. ${describeError(reason).message}`),
  })
  const headingId = `workos-heading-${connection.id}`

  if (bindings.isPending) return <ListSkeleton label="Loading single sign-on" rows={1} />
  if (bindings.isError) {
    return (
      <Banner onRetry={() => void bindings.refetch()} title="Single sign-on for this agent didn’t load." tone="danger">
        {describeLoadError(bindings.error)}
      </Banner>
    )
  }
  // Nothing to do here until whoever runs the server turns it on.
  if (!bindings.data.enabled) return null
  const active = bindings.data.bindings.filter((binding) => !binding.revokedAt)
  const pending = update.isPending ? update.variables : null
  return (
    <section aria-labelledby={headingId} className="aw-webhook">
      <h3 className="aw-row__title" id={headingId}>
        Single sign-on
      </h3>
      <p className="aw-quiet">
        Lets the agent use short-lived tokens from your organization’s sign-in, with the same account and projects.{' '}
        <a className="aw-textlink" href="/auth.md" rel="noreferrer" target="_blank">
          How agents sign in
        </a>
      </p>
      {error ? <Banner tone="danger">{error}</Banner> : null}
      {active.length ? (
        <RowList label={`Single sign-on for ${connection.displayName}`}>
          {active.map((binding) => (
            <Row
              actions={
                <Button
                  busy={pending?.revoke === binding.id}
                  disabled={update.isPending}
                  onClick={() => update.mutate({ revoke: binding.id })}
                  size="sm"
                  variant="ghost"
                >
                  Revoke
                </Button>
              }
              key={binding.id}
              title={binding.kind === 'blueprint' ? 'Short-lived tokens on' : 'Registered agent attached'}
            >
              <details className="aw-disclosure">
                <summary>Details</summary>
                <code>{binding.subjectId}</code>
              </details>
            </Row>
          ))}
        </RowList>
      ) : null}
      <div className="aw-card__actions">
        <Button busy={pending != null && !pending.revoke && !pending.registrationId} disabled={update.isPending} onClick={() => update.mutate({})} size="sm">
          {active.some((binding) => binding.kind === 'blueprint') ? 'Reconnect after a new token' : 'Turn on short-lived tokens'}
        </Button>
      </div>
      <p className="aw-quiet">
        The agent trades its token for one that lasts five minutes. After you make it a new token, reconnect here. Disabling
        the agent stops both kinds of token.
      </p>
      {bindings.data.registrationEnabled ? (
        <>
          <TextField
            hint="For an agent that registered itself with your organization’s sign-in. Link your single sign-on account and claim the registration first."
            id={`workos-registration-${connection.id}`}
            label="Registration ID"
            maxLength={100}
            onChange={(event) => setRegistrationId(event.target.value)}
            placeholder="agent_reg_…"
            value={registrationId}
          />
          <div className="aw-card__actions">
            <Button
              busy={Boolean(pending?.registrationId)}
              disabled={update.isPending || !registrationId.startsWith('agent_reg_')}
              onClick={() => update.mutate({ registrationId })}
              size="sm"
            >
              Attach registered agent
            </Button>
          </div>
        </>
      ) : null}
    </section>
  )
}
