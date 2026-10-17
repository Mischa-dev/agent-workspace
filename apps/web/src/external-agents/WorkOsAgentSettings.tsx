import { useState } from 'react'
import { useMutation, useQuery, useQueryClient } from '@tanstack/react-query'

import { requestJson } from '../api/client'
import { isApiError } from '../api/errors'
import { Banner, Button, ListSkeleton, Row, RowList, TextField } from '../ui'
import type { ExternalAgentConnection } from './api'

type Binding = { id: string; kind: 'blueprint' | 'registration'; subjectId: string; revokedAt: string | null }

/** WorkOS sign-in for one agent: short-lived tokens in place of its connection token. */
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
    onError: (reason) => setError(isApiError(reason) ? reason.message : 'Agent sign-in wasn’t updated. Try again.'),
  })
  const headingId = `workos-heading-${connection.id}`

  if (bindings.isPending) return <ListSkeleton label="Loading WorkOS sign-in" rows={1} />
  if (bindings.isError) {
    return (
      <Banner
        action={
          <Button onClick={() => void bindings.refetch()} size="sm">
            Try again
          </Button>
        }
        tone="danger"
      >
        WorkOS sign-in for this agent didn’t load.
      </Banner>
    )
  }
  if (!bindings.data.enabled) return <p className="aw-quiet">WorkOS sign-in for agents isn’t set up on this host.</p>
  const active = bindings.data.bindings.filter((binding) => !binding.revokedAt)
  const pending = update.isPending ? update.variables : null
  return (
    <section aria-labelledby={headingId} className="aw-webhook">
      <h3 className="aw-row__title" id={headingId}>
        WorkOS sign-in
      </h3>
      <p className="aw-quiet">
        Use short-lived tokens with this agent’s existing account and project memberships.{' '}
        <a className="aw-textlink" href="/auth.md" rel="noreferrer" target="_blank">
          How agents sign in
        </a>
      </p>
      {error ? <Banner tone="danger">{error}</Banner> : null}
      {active.length ? (
        <RowList label={`WorkOS sign-ins for ${connection.displayName}`}>
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
              meta={<code>{binding.subjectId}</code>}
              title={binding.kind === 'blueprint' ? 'Agent tokens on' : 'External registration attached'}
            />
          ))}
        </RowList>
      ) : null}
      <div className="aw-card__actions">
        <Button busy={pending != null && !pending.revoke && !pending.registrationId} disabled={update.isPending} onClick={() => update.mutate({})} size="sm">
          {active.some((binding) => binding.kind === 'blueprint') ? 'Reconnect after token rotation' : 'Turn on WorkOS tokens'}
        </Button>
      </div>
      <p className="aw-quiet">
        The agent trades its connection token for a five-minute WorkOS token. After you rotate the connection token,
        reconnect here. Turning the agent off stops both kinds of token.
      </p>
      {bindings.data.registrationEnabled ? (
        <>
          <TextField
            hint="For an agent following auth.md. Link your WorkOS account and claim the registration first."
            id={`workos-registration-${connection.id}`}
            label="Claimed WorkOS registration ID"
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
              Attach claimed agent
            </Button>
          </div>
        </>
      ) : null}
    </section>
  )
}
