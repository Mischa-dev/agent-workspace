import { useState } from 'react'
import { useMutation, useQuery, useQueryClient } from '@tanstack/react-query'
import { requestJson } from '../api/client'
import { isApiError } from '../api/errors'
import { Banner, Button, TextField } from '../ui'
import type { ExternalAgentConnection } from './api'

type Binding = { id: string; kind: 'blueprint' | 'registration'; subjectId: string; revokedAt: string | null }

export function WorkOsAgentSettings({ connection }: { connection: ExternalAgentConnection }) {
  const path = `/api/v1/projects/${encodeURIComponent(connection.projectId)}/external-agents/${encodeURIComponent(connection.id)}/workos`
  const client = useQueryClient()
  const [registrationId, setRegistrationId] = useState('')
  const [error, setError] = useState<string | null>(null)
  const queryKey = ['workos-agent', connection.id]
  const bindings = useQuery({ queryKey, queryFn: () => requestJson<{ enabled: boolean; registrationEnabled: boolean; bindings: Binding[] }>(path) })
  const update = useMutation({
    mutationFn: (input: { registrationId?: string; revoke?: string }) => input.revoke
      ? requestJson(`${path}/${encodeURIComponent(input.revoke)}`, { method: 'DELETE' })
      : requestJson(path, { method: 'POST', json: input }),
    onSuccess: async () => { setError(null); setRegistrationId(''); await client.invalidateQueries({ queryKey }) },
    onError: (reason) => setError(isApiError(reason) ? reason.message : 'Could not update agent authentication.'),
  })
  if (bindings.isPending) return <p>Loading agent authentication…</p>
  if (bindings.isError) return <Banner tone="danger">Could not load WorkOS agent authentication.</Banner>
  if (!bindings.data.enabled) return <p>WorkOS agent authentication is not configured on this host.</p>
  const active = bindings.data.bindings.filter((binding) => !binding.revokedAt)
  return (
    <section aria-label={`WorkOS authentication for ${connection.displayName}`}>
      <h3>WorkOS agent authentication</h3>
      <p>Use short-lived tokens with this agent’s existing account and project memberships. <a href="/auth.md" target="_blank" rel="noreferrer">Agent login instructions</a></p>
      {error && <Banner tone="danger">{error}</Banner>}
      {active.map((binding) => <div key={binding.id}>
        <p>{binding.kind === 'blueprint' ? 'Agent tokens enabled' : 'External registration attached'}: <code>{binding.subjectId}</code></p>
        <Button busy={update.isPending} onClick={() => update.mutate({ revoke: binding.id })}>Revoke WorkOS access</Button>
      </div>)}
      <Button busy={update.isPending} onClick={() => update.mutate({})}>{active.some((b) => b.kind === 'blueprint') ? 'Reconnect after token rotation' : 'Enable WorkOS agent tokens'}</Button>
      <p>Your agent exchanges its current connection token for a five-minute WorkOS token. After rotating the connection token, reconnect here. Disabling the agent stops both credential types.</p>
      {bindings.data.registrationEnabled && <>
        <TextField id={`workos-registration-${connection.id}`} label="Claimed WorkOS registration ID" placeholder="agent_reg_…" value={registrationId} maxLength={100} onChange={(event) => setRegistrationId(event.target.value)} hint="For an external agent following auth.md. Link your WorkOS account and claim the registration first." />
        <Button busy={update.isPending} disabled={!registrationId.startsWith('agent_reg_')} onClick={() => update.mutate({ registrationId })}>Attach claimed agent</Button>
      </>}
    </section>
  )
}
