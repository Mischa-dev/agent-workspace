import { useState, type FormEvent } from 'react'
import { useMutation, useQuery, useQueryClient } from '@tanstack/react-query'

import { describeError } from '../../app/mutations'
import type { OrganizationSummary } from '../../auth/session'
import {
  Banner,
  Button,
  ConfirmDialog,
  EmptyState,
  ErrorState,
  Icon,
  ListSkeleton,
  PageHeader,
  Row,
  RowList,
  Section,
  SelectField,
  StatusChip,
  TextField,
  formatFullDate,
  formatRelative,
  showToast,
  type ChipTone,
} from '../../ui'
import { OneTimeValue } from '../OneTimeValue'
import { actorKind } from '../../ui/identityModel'
import { listWorkspaceMembers, workspaceKeys } from '../workspace/api'
import {
  bindHostAgent,
  createEnrolment,
  hostKeys,
  listHosts,
  unbindHostAgent,
  unlinkHost,
  type EnrolmentCode,
  type Host,
} from './api'

const STATUS: Record<Host['status'], { label: string; tone: ChipTone }> = {
  active: { label: 'Linked', tone: 'success' },
  offline: { label: 'Offline', tone: 'warning' },
  revoked: { label: 'Unlinked', tone: 'muted' },
}

/**
 * A1.11: Settings › Hosts. A host is a machine or service that runs agents.
 * Linking one makes a one-time code its administrator gives to the host's own
 * enrol step; then the host's agents are bound to agent accounts here. Panels
 * a host offers (M1.12) are shown as plain lists until modules can draw them.
 */
export function HostsView({ mode, workspace }: { mode: 'workspace' | 'account'; workspace: OrganizationSummary | null }) {
  const queryClient = useQueryClient()
  // Every call acts in the current workspace; the account page lists only
  // the hosts you own.
  const workspaceId = workspace?.id ?? null
  const mineOnly = mode === 'account'
  const key = hostKeys.list(workspaceId, mineOnly)
  const hosts = useQuery({ queryKey: key, queryFn: () => listHosts(workspaceId, mineOnly) })
  const members = useQuery({
    queryKey: workspace ? workspaceKeys.members(workspace.id) : ['organization', 'none', 'members'],
    queryFn: () => listWorkspaceMembers(workspace?.id ?? ''),
    enabled: Boolean(workspace),
  })
  const agents = (members.data?.members ?? []).filter((member) => actorKind(member.kind) === 'agent' && member.enabled)
  const [code, setCode] = useState<EnrolmentCode | null>(null)
  const [label, setLabel] = useState('')
  const [unlinking, setUnlinking] = useState<Host | null>(null)
  const refresh = () => queryClient.invalidateQueries({ queryKey: key })
  const enrol = useMutation({
    mutationFn: () =>
      createEnrolment(workspaceId, {
        owner: mode === 'workspace' ? 'workspace' : 'person',
        ...(label.trim() ? { label: label.trim() } : {}),
      }),
    onSuccess: (made) => {
      setCode(made)
      setLabel('')
    },
  })
  const unlink = useMutation({
    mutationFn: (host: Host) => unlinkHost(workspaceId, host.id),
    onSuccess: (host) => {
      setUnlinking(null)
      showToast({ tone: 'success', title: `${host.name} unlinked`, description: 'Its credential stopped working.' })
    },
    onSettled: refresh,
  })
  const owner = mode === 'workspace' ? (workspace?.name ?? 'the workspace') : 'you'

  return (
    <>
      <PageHeader icon="terminal" title="Hosts">
        Hosts are machines and services that run agents, such as a computer of yours or the team’s server. A host acts
        only for the agents bound to it here, and never chooses which.
      </PageHeader>
      {code && (
        <OneTimeValue
          icon="key"
          onDone={() => {
            setCode(null)
            void refresh()
          }}
          title="Give this code to the host"
          value={code.code}
        >
          Run the host’s own enrol step with this code and this server’s address. It works once, until{' '}
          {formatFullDate(code.expiresAt)}, and links a host owned by {code.owner.name}.
        </OneTimeValue>
      )}
      <Section id="link-host-heading" title="Link a host">
        <form
          className="aw-card aw-formcard aw-form"
          onSubmit={(event: FormEvent<HTMLFormElement>) => {
            event.preventDefault()
            enrol.mutate()
          }}
        >
          <TextField
            hint={`A note for you, such as the machine’s name. The host will belong to ${owner}.`}
            id="host-label"
            label="Label"
            maxLength={80}
            onChange={(event) => setLabel(event.target.value)}
            value={label}
          />
          {enrol.isError && <Banner tone="danger">{`No code was made. ${describeError(enrol.error).message}`}</Banner>}
          <div className="aw-form__actions">
            <Button busy={enrol.isPending} icon="plus" type="submit" variant="primary">
              Make a code
            </Button>
          </div>
        </form>
      </Section>
      <Section id="hosts-heading" title={mode === 'workspace' ? 'Hosts here' : 'Your hosts'}>
        {hosts.isPending ? (
          <ListSkeleton label="Loading hosts" rows={2} />
        ) : hosts.isError ? (
          <ErrorState error={hosts.error} onRetry={() => void hosts.refetch()} title="Hosts didn’t load" />
        ) : hosts.data.hosts.length ? (
          <RowList label="Hosts">
            {hosts.data.hosts.map((host) => (
              <HostRow
                agents={agents.map((agent) => ({ id: agent.actorId, name: agent.displayName }))}
                host={host}
                key={host.id}
                onChanged={() => void refresh()}
                onUnlink={() => setUnlinking(host)}
                workspaceId={workspaceId}
              />
            ))}
          </RowList>
        ) : (
          <EmptyState icon="terminal" title="No hosts yet">
            Make a code above, then run the host’s enrol step with it.
          </EmptyState>
        )}
      </Section>
      <ConfirmDialog
        busy={unlink.isPending}
        confirmLabel="Unlink host"
        error={unlink.isError ? `The host wasn’t unlinked. ${describeError(unlink.error).message}` : null}
        onClose={() => {
          setUnlinking(null)
          unlink.reset()
        }}
        onConfirm={() => unlinking && unlink.mutate(unlinking)}
        open={unlinking !== null}
        title={`Unlink ${unlinking?.name ?? 'this host'}?`}
      >
        Its credential stops working at once, its runs here are interrupted and what it asked people for is withdrawn.
        Linking it again takes a new code.
      </ConfirmDialog>
    </>
  )
}

function HostRow({
  agents,
  host,
  onChanged,
  onUnlink,
  workspaceId,
}: {
  agents: { id: string; name: string }[]
  host: Host
  onChanged: () => void
  onUnlink: () => void
  workspaceId: string | null
}) {
  const bound = new Set(host.agents.map((agent) => agent.hostAgentRef))
  const unbound = host.describedAgents.filter((agent) => !bound.has(agent.hostAgentRef))
  const [reference, setReference] = useState('')
  const [accountId, setAccountId] = useState('')
  const bind = useMutation({
    mutationFn: () => bindHostAgent(workspaceId, host.id, accountId, reference),
    onSuccess: () => {
      setReference('')
      setAccountId('')
      onChanged()
    },
  })
  const unbind = useMutation({
    mutationFn: (account: string) => unbindHostAgent(workspaceId, host.id, account),
    onSuccess: onChanged,
  })
  return (
    <Row
      actions={
        host.canManage ? (
          <Button icon="x" onClick={onUnlink} size="sm" variant="ghost">
            Unlink
          </Button>
        ) : null
      }
      leading={
        <span className="aw-row__tile">
          <Icon name="terminal" size={16} />
        </span>
      }
      meta={
        <div className="aw-stack">
          <span className="aw-row__metaline aw-wrap">
            <span>{host.builtin ? 'This server' : `Owned by ${host.owner.name}`}</span>
            {host.software && <span>{[host.software, host.softwareVersion].filter(Boolean).join(' ')}</span>}
            {host.lastSeenAt ? (
              <span title={formatFullDate(host.lastSeenAt)}>Seen {formatRelative(host.lastSeenAt)}</span>
            ) : (
              <span>Not seen yet</span>
            )}
          </span>
          {host.agents.length > 0 && (
            <ul className="aw-plain-list" aria-label={`Agents on ${host.name}`}>
              {host.agents.map((agent) => (
                <li key={agent.accountId}>
                  {agent.name ?? 'An agent'} as <code className="aw-inline-code">{agent.hostAgentRef}</code>
                  {agent.status !== 'active' && <> ({agent.status})</>}
                  {host.canManage && (
                    <Button
                      disabled={unbind.isPending}
                      onClick={() => unbind.mutate(agent.accountId)}
                      size="sm"
                      variant="ghost"
                    >
                      Unbind
                    </Button>
                  )}
                </li>
              ))}
            </ul>
          )}
          {host.offers.length > 0 && (
            <span className="aw-quiet">Offers: {host.offers.map((offer) => offer.title).join(', ')}</span>
          )}
          {host.canManage && unbound.length > 0 && agents.length > 0 && (
            <form
              className="aw-form aw-form--inline"
              onSubmit={(event: FormEvent<HTMLFormElement>) => {
                event.preventDefault()
                if (reference && accountId) bind.mutate()
              }}
            >
              <SelectField
                id={`host-${host.id}-ref`}
                label="Host’s agent"
                onChange={(event) => setReference(event.target.value)}
                value={reference}
              >
                <option value="">Choose…</option>
                {unbound.map((agent) => (
                  <option key={agent.hostAgentRef} value={agent.hostAgentRef}>
                    {agent.name ?? agent.hostAgentRef}
                  </option>
                ))}
              </SelectField>
              <SelectField
                id={`host-${host.id}-account`}
                label="Acts as"
                onChange={(event) => setAccountId(event.target.value)}
                value={accountId}
              >
                <option value="">Choose…</option>
                {agents.map((agent) => (
                  <option key={agent.id} value={agent.id}>
                    {agent.name}
                  </option>
                ))}
              </SelectField>
              <Button busy={bind.isPending} disabled={!reference || !accountId} type="submit">
                Bind
              </Button>
              {bind.isError && <Banner tone="danger">{describeError(bind.error).message}</Banner>}
            </form>
          )}
        </div>
      }
      title={
        <span className="aw-row__titleline">
          {host.name}
          <StatusChip tone={STATUS[host.status].tone}>{STATUS[host.status].label}</StatusChip>
        </span>
      }
    />
  )
}
