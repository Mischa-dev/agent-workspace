import { useState, type FormEvent } from 'react'
import { useQuery } from '@tanstack/react-query'

import { describeError, toastError } from '../../app/mutations'
import { useWorkspaceId } from '../../app/workspace'
import { opQuery, useOp } from '../../api/ops'
import type { ConnectionView, DeliveryView, Entry } from '../../api/generated/operations'
import { useProjects } from '../../projects/hooks'
import {
  Banner,
  Button,
  Checkbox,
  ConfirmDialog,
  Dialog,
  EmptyState,
  ErrorState,
  Icon,
  IconButton,
  ListSkeleton,
  Menu,
  PageHeader,
  Row,
  RowList,
  Section,
  SelectField,
  StatusChip,
  TextField,
  countLabel,
  formatFullDate,
  formatRelative,
  showToast,
  type ChipTone,
} from '../../ui'
import '../../automations/automations.css'
import { OneTimeValue } from '../OneTimeValue'

const DELIVERY: Record<string, { label: string; tone: ChipTone }> = {
  pending: { label: 'Sending', tone: 'muted' },
  delivered: { label: 'Delivered', tone: 'success' },
  failed: { label: 'Failed', tone: 'danger' },
  skipped: { label: 'Skipped', tone: 'muted' },
}

const SIGNING =
  'Each delivery is a POST of JSON carrying event ids, never content: fetch what changed with your own access. ' +
  'It is signed the Standard Webhooks way: check the webhook-signature header, an HMAC-SHA256 of ' +
  '"webhook-id.webhook-timestamp.body" keyed with the base64-decoded part of your signing secret after whsec_, and refuse old timestamps. ' +
  'A delivery sent again keeps its webhook-id, so the receiver can tell.'

/** A webhook of yours is a connection with an address: the events it hears are its subscription. */
function isWebhook(connection: ConnectionView) {
  return Boolean(connection.webhookUrl)
}

function kindsOf(connection: ConnectionView) {
  return [...new Set(connection.subscriptions.flatMap((subscription) => subscription.kinds))]
}

function spaceOf(connection: ConnectionView) {
  return connection.subscriptions[0]?.spaceIds?.[0] ?? ''
}

/** Account › Webhooks: signed POSTs to your own services when things you can see change. */
export default function WebhooksPage() {
  const workspaceId = useWorkspaceId() ?? undefined
  const list = useQuery(opQuery('connection_list', {}, { workspaceId }))
  const kinds = useQuery({ ...opQuery('event_kind_list', {}, { workspaceId }), staleTime: 5 * 60_000 })
  const projects = useProjects()
  const [secret, setSecret] = useState<{ name: string; value: string } | null>(null)
  const [editing, setEditing] = useState<ConnectionView | 'new' | null>(null)
  const [deleting, setDeleting] = useState<ConnectionView | null>(null)
  const [replacing, setReplacing] = useState<ConnectionView | null>(null)
  const [log, setLog] = useState<ConnectionView | null>(null)
  const invalidates = ['connection_list' as const]

  const turnOn = useOp('connection_update', {
    workspaceId,
    invalidates,
    onSuccess: ({ connection }) => showToast({ title: `${connection.name} is on` }),
  })
  const rotate = useOp('connection_secret_rotate', {
    workspaceId,
    invalidates,
    onSuccess: ({ connection, webhookSecret }) => {
      setReplacing(null)
      if (webhookSecret) setSecret({ name: connection.name, value: webhookSecret })
    },
  })
  const ping = useOp('connection_test', {
    workspaceId,
    invalidates,
    onSuccess: () =>
      showToast({
        tone: 'success',
        title: 'Test sent',
        description: 'How it went shows on the webhook in a moment.',
      }),
  })
  const remove = useOp('connection_remove', {
    workspaceId,
    invalidates,
    onSuccess: (connection) => {
      setDeleting(null)
      showToast({ title: `${connection.name} deleted` })
    },
  })
  const webhooks = list.data?.connections.filter(isWebhook) ?? []
  const projectName = (id: string) => projects.data?.projects.find((project) => project.id === id)?.name ?? 'One project'

  return (
    <>
      <PageHeader
        actions={
          list.data ? (
            <Button icon="plus" onClick={() => setEditing('new')} variant="primary">
              New webhook
            </Button>
          ) : null
        }
        icon="send"
        title="Webhooks"
      >
        Send events to your own services as signed JSON. A webhook hears only what you can see, and stops hearing what you lose access to.
      </PageHeader>
      {secret && (
        <OneTimeValue onDone={() => setSecret(null)} title={`Signing secret for ${secret.name}`} value={secret.value}>
          Copy it into the receiving service now. It can’t be shown again; replace it if you lose it.
        </OneTimeValue>
      )}
      {list.isPending ? (
        <ListSkeleton label="Loading webhooks" rows={2} />
      ) : list.isError ? (
        <ErrorState error={list.error} onRetry={() => void list.refetch()} title="Webhooks didn’t load" />
      ) : (
        <>
          <Section id="webhooks-heading" title="Your webhooks">
            {webhooks.length ? (
              <RowList label="Webhooks">
                {webhooks.map((webhook) => {
                  const on = webhook.state === 'active'
                  const space = spaceOf(webhook)
                  return (
                    <Row
                      actions={
                        <>
                          <Button onClick={() => setLog(webhook)} size="sm" variant="ghost">
                            Deliveries
                          </Button>
                          <Menu
                            items={[
                              { label: 'Edit', icon: 'pencil', onSelect: () => setEditing(webhook) },
                              {
                                label: 'Send a test',
                                icon: 'send',
                                onSelect: () =>
                                  ping.mutate(
                                    { connectionId: webhook.id },
                                    { onError: (error) => toastError('The test wasn’t sent', error) },
                                  ),
                              },
                              ...(on
                                ? []
                                : [
                                    {
                                      label: 'Turn back on',
                                      icon: 'play' as const,
                                      onSelect: () =>
                                        turnOn.mutate(
                                          {
                                            connectionId: webhook.id,
                                            webhookUrl: webhook.webhookUrl,
                                            expectedVersion: webhook.version,
                                          },
                                          { onError: (error) => toastError('The webhook didn’t turn on', error) },
                                        ),
                                    },
                                  ]),
                              { label: 'Replace the secret', icon: 'key', onSelect: () => setReplacing(webhook) },
                              { separator: true },
                              { label: 'Delete', icon: 'trash', danger: true, onSelect: () => setDeleting(webhook) },
                            ]}
                            label={`${webhook.name} actions`}
                            trigger={<IconButton icon="more" label={`${webhook.name} actions`} size="sm" />}
                          />
                        </>
                      }
                      className={on ? undefined : 'is-disabled'}
                      key={webhook.id}
                      leading={
                        <span className="aw-row__tile">
                          <Icon name="send" size={16} />
                        </span>
                      }
                      meta={
                        <span className="aw-row__metaline aw-wrap">
                          <code className="aw-inline-code">{webhook.webhookUrl}</code>
                          <span>{space ? projectName(space) : 'Everything you can see here'}</span>
                          <span>{kindsOf(webhook).join(', ') || 'Every event'}</span>
                          <span title={webhook.lastDeliveredAt ? formatFullDate(webhook.lastDeliveredAt) : undefined}>
                            {webhook.lastDeliveredAt ? `Last sent ${formatRelative(webhook.lastDeliveredAt)}` : 'Nothing sent yet'}
                          </span>
                          {webhook.lastTest && (
                            <span title={formatFullDate(webhook.lastTest.at)}>
                              {webhook.lastTest.ok
                                ? `Test answered ${webhook.lastTest.status ?? 'OK'}`
                                : `Test failed${webhook.lastTest.status ? ` (HTTP ${webhook.lastTest.status})` : ''}${webhook.lastTest.error ? `: ${webhook.lastTest.error}` : ''}`}
                            </span>
                          )}
                          {webhook.madeThroughApp && <span>Set up by an app</span>}
                          {webhook.disabledReason && <span>{webhook.disabledReason}</span>}
                        </span>
                      }
                      title={
                        <span className="aw-row__titleline">
                          {webhook.name}
                          <StatusChip tone={on ? 'success' : 'danger'}>{on ? 'On' : 'Stopped'}</StatusChip>
                        </span>
                      }
                    />
                  )
                })}
              </RowList>
            ) : (
              <EmptyState
                action={
                  <Button icon="plus" onClick={() => setEditing('new')}>
                    New webhook
                  </Button>
                }
                icon="send"
                title="No webhooks yet"
              >
                Hear about finished runs, failing checks, new tasks and messages in your own tools.
              </EmptyState>
            )}
          </Section>
          <Section id="webhook-signing-heading" title="Checking a delivery">
            <p className="aw-quiet">{SIGNING}</p>
          </Section>
        </>
      )}
      {editing && (
        <WebhookDialog
          kinds={(kinds.data?.events ?? []).filter((entry) => !entry.addressed)}
          onClose={() => setEditing(null)}
          onSaved={(result) => {
            setEditing(null)
            if (result.secret) setSecret({ name: result.name, value: result.secret })
          }}
          projects={(projects.data?.projects ?? []).filter(
            (project) => !project.archivedAt && project.organizationId === workspaceId && project.kind !== 'workspace',
          )}
          webhook={editing === 'new' ? null : editing}
          workspaceId={workspaceId}
        />
      )}
      {log && <DeliveriesDialog onClose={() => setLog(null)} webhook={log} workspaceId={workspaceId} />}
      <ConfirmDialog
        busy={remove.isPending}
        confirmLabel="Delete webhook"
        error={remove.isError ? `The webhook wasn’t deleted. ${describeError(remove.error).message}` : null}
        onClose={() => {
          setDeleting(null)
          remove.reset()
        }}
        onConfirm={() => deleting && remove.mutate({ connectionId: deleting.id })}
        open={deleting !== null}
        title={`Delete ${deleting?.name ?? 'this webhook'}?`}
      >
        It stops sending events right away, and its delivery log goes too.
      </ConfirmDialog>
      <ConfirmDialog
        busy={rotate.isPending}
        confirmLabel="Replace secret"
        error={rotate.isError ? `The secret wasn’t replaced. ${describeError(rotate.error).message}` : null}
        onClose={() => {
          setReplacing(null)
          rotate.reset()
        }}
        onConfirm={() => replacing && rotate.mutate({ connectionId: replacing.id })}
        open={replacing !== null}
        title={`Replace the secret for ${replacing?.name ?? 'this webhook'}?`}
      >
        Deliveries signed with the old secret will fail until the receiving service has the new one.
      </ConfirmDialog>
    </>
  )
}

function WebhookDialog({
  kinds,
  onClose,
  onSaved,
  projects,
  webhook,
  workspaceId,
}: {
  kinds: Entry[]
  onClose: () => void
  onSaved: (result: { name: string; secret: string | null }) => void
  projects: { id: string; name: string }[]
  webhook: ConnectionView | null
  workspaceId?: string
}) {
  const initialEvents = webhook ? kindsOf(webhook) : ['run.finished']
  const initialSpace = webhook ? spaceOf(webhook) : ''
  const [name, setName] = useState(webhook?.name ?? '')
  const [url, setUrl] = useState(webhook?.webhookUrl ?? 'https://')
  const [spaceId, setSpaceId] = useState(initialSpace)
  const [events, setEvents] = useState<string[]>(initialEvents)
  const [error, setError] = useState<string | null>(null)
  const onError = (reason: unknown) => setError(`The webhook wasn’t saved. ${describeError(reason).message}`)
  const done = ({ connection, webhookSecret }: { connection: ConnectionView; webhookSecret?: string | null }) =>
    onSaved({ name: connection.name, secret: webhookSecret ?? null })
  const create = useOp('connection_create', { workspaceId, invalidates: ['connection_list'], onSuccess: done })
  const update = useOp('connection_update', { workspaceId, invalidates: ['connection_list'], onSuccess: done })
  const subscriptions = () => [
    {
      kinds: events,
      workspaceIds: workspaceId ? [workspaceId] : null,
      spaceIds: spaceId ? [spaceId] : null,
      includesDms: true,
      echoes: true,
      minReason: 'visible',
    },
  ]
  const submit = (event: FormEvent<HTMLFormElement>) => {
    event.preventDefault()
    if (!name.trim() || !url.trim() || !events.length) return
    setError(null)
    if (webhook) {
      update.mutate(
        {
          connectionId: webhook.id,
          name: name.trim(),
          webhookUrl: url.trim(),
          subscriptions: subscriptions(),
          expectedVersion: webhook.version,
        },
        { onError },
      )
    } else {
      create.mutate(
        {
          name: name.trim(),
          webhookUrl: url.trim(),
          reachesAll: false,
          subscriptions: subscriptions(),
        },
        { onError },
      )
    }
  }
  const dirty =
    name !== (webhook?.name ?? '') ||
    url !== (webhook?.webhookUrl ?? 'https://') ||
    spaceId !== initialSpace ||
    events.join() !== initialEvents.join()

  return (
    <Dialog
      footer={
        <>
          <Button onClick={onClose}>Cancel</Button>
          <Button
            busy={create.isPending || update.isPending}
            disabled={!name.trim() || !events.length}
            type="submit"
            variant="primary"
          >
            {webhook ? 'Save changes' : 'Create webhook'}
          </Button>
        </>
      }
      dirty={dirty}
      onClose={onClose}
      onSubmit={submit}
      open
      title={webhook ? `Edit ${webhook.name}` : 'New webhook'}
    >
      <div className="aw-form">
        <TextField id="webhook-name" label="Name" maxLength={80} onChange={(event) => setName(event.target.value)} required value={name} />
        <TextField
          hint="An https address. Private and local addresses are refused unless the server trusts them."
          id="webhook-url"
          label="URL"
          mono
          onChange={(event) => setUrl(event.target.value)}
          required
          type="url"
          value={url}
        />
        <SelectField id="webhook-project" label="Hear from" onChange={(event) => setSpaceId(event.target.value)} value={spaceId}>
          <option value="">Everything you can see in this workspace</option>
          {projects.map((project) => (
            <option key={project.id} value={project.id}>
              {project.name}
            </option>
          ))}
        </SelectField>
        <fieldset className="aw-fieldset">
          <legend className="aw-field__label">Events</legend>
          {kinds.map((kind) => (
            <Checkbox
              checked={events.includes(kind.kind)}
              description={kind.summary}
              key={kind.kind}
              label={<code className="aw-inline-code">{kind.kind}</code>}
              onChange={(checked) =>
                setEvents((current) => (checked ? [...current, kind.kind] : current.filter((name) => name !== kind.kind)))
              }
            />
          ))}
        </fieldset>
        {error && <Banner tone="danger">{error}</Banner>}
      </div>
    </Dialog>
  )
}

function DeliveriesDialog({
  onClose,
  webhook,
  workspaceId,
}: {
  onClose: () => void
  webhook: ConnectionView
  workspaceId?: string
}) {
  const deliveries = useQuery(opQuery('connection_delivery_list', { connectionId: webhook.id }, { workspaceId }))
  const again = useOp('connection_redeliver', {
    workspaceId,
    invalidates: ['connection_delivery_list'],
    onSuccess: () =>
      showToast({
        title: 'Sending it again',
        description: 'It keeps its delivery id, so the receiver can tell.',
      }),
  })
  return (
    <Dialog footer={<Button onClick={onClose}>Done</Button>} onClose={onClose} open title={`Deliveries for ${webhook.name}`}>
      {deliveries.isPending ? (
        <ListSkeleton label="Loading deliveries" rows={3} />
      ) : deliveries.isError ? (
        <ErrorState error={deliveries.error} onRetry={() => void deliveries.refetch()} title="Deliveries didn’t load" />
      ) : deliveries.data.deliveries.length ? (
        <RowList label="Deliveries">
          {deliveries.data.deliveries.map((delivery: DeliveryView) => {
            const shown = DELIVERY[delivery.state] ?? { label: delivery.state, tone: 'muted' as const }
            return (
              <Row
                actions={
                  delivery.state === 'pending' ? null : (
                    <Button
                      disabled={again.isPending}
                      onClick={() =>
                        again.mutate(
                          { connectionId: webhook.id, eventPosition: delivery.eventPosition },
                          { onError: (error) => toastError('It wasn’t sent again', error) },
                        )
                      }
                      size="sm"
                      variant="ghost"
                    >
                      Send again
                    </Button>
                  )
                }
                key={delivery.eventId}
                meta={
                  <span className="aw-row__metaline aw-wrap">
                    <span>
                      Delivery <code className="aw-inline-code">{delivery.eventId}</code>
                    </span>
                    <span title={formatFullDate(delivery.createdAt)}>{formatRelative(delivery.createdAt)}</span>
                    <span>{countLabel(delivery.attempts, 'attempt')}</span>
                    {delivery.lastStatus != null && <span>HTTP {delivery.lastStatus}</span>}
                    {delivery.lastError && <span>{delivery.lastError}</span>}
                  </span>
                }
                title={
                  <span className="aw-row__titleline">
                    <code className="aw-inline-code">{delivery.eventType}</code>
                    <StatusChip tone={shown.tone}>{shown.label}</StatusChip>
                  </span>
                }
              />
            )
          })}
        </RowList>
      ) : (
        <p className="aw-quiet">Nothing sent yet. Send a test to try it.</p>
      )}
    </Dialog>
  )
}
