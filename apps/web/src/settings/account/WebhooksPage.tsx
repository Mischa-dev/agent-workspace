import { useState, type FormEvent } from 'react'
import { useMutation, useQuery, useQueryClient } from '@tanstack/react-query'

import { describeError, toastError } from '../../app/mutations'
import {
  automationKeys,
  createWebhook,
  deleteWebhook,
  listDeliveries,
  listWebhooks,
  redeliver,
  rotateWebhookSecret,
  testWebhook,
  updateWebhook,
  type Webhook,
  type WebhookDelivery,
  type WebhookList,
} from '../../automations/api'
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

const STATE: Record<Webhook['state'], { label: string; tone: ChipTone }> = {
  active: { label: 'On', tone: 'success' },
  paused: { label: 'Paused', tone: 'muted' },
  disabled: { label: 'Turned off', tone: 'danger' },
}

const DELIVERY: Record<WebhookDelivery['state'], { label: string; tone: ChipTone }> = {
  pending: { label: 'Sending', tone: 'muted' },
  delivered: { label: 'Delivered', tone: 'success' },
  failed: { label: 'Failed', tone: 'danger' },
  skipped: { label: 'Skipped', tone: 'muted' },
}

/** Account › Webhooks: signed POSTs to your own services when tasks, runs, pull requests or messages change. */
export default function WebhooksPage() {
  const queryClient = useQueryClient()
  const list = useQuery({
    queryKey: automationKeys.webhooks,
    queryFn: listWebhooks,
  })
  const [secret, setSecret] = useState<{ name: string; value: string } | null>(null)
  const [editing, setEditing] = useState<Webhook | 'new' | null>(null)
  const [deleting, setDeleting] = useState<Webhook | null>(null)
  const [replacing, setReplacing] = useState<Webhook | null>(null)
  const [log, setLog] = useState<Webhook | null>(null)
  const refresh = () => queryClient.invalidateQueries({ queryKey: automationKeys.webhooks })

  const toggle = useMutation({
    mutationFn: (webhook: Webhook) =>
      updateWebhook(webhook, {
        state: webhook.state === 'active' ? 'paused' : 'active',
      }),
    onSuccess: (webhook): void => {
      showToast({
        title: webhook.state === 'active' ? `${webhook.name} is on` : `${webhook.name} paused`,
        onUndo: () => toggle.mutate(webhook),
      })
    },
    onError: (error) => toastError('The webhook didn’t change', error),
    onSettled: refresh,
  })
  const rotate = useMutation({
    mutationFn: (webhook: Webhook) => rotateWebhookSecret(webhook.id),
    onSuccess: (result) => {
      setReplacing(null)
      setSecret({ name: result.webhook.name, value: result.secret })
    },
    onSettled: refresh,
  })
  const ping = useMutation({
    mutationFn: (webhook: Webhook) => testWebhook(webhook.id),
    onSuccess: () =>
      showToast({
        tone: 'success',
        title: 'Test sent',
        description: 'Open its deliveries to see how it went.',
      }),
    onError: (error) => toastError('The test wasn’t sent', error),
  })
  const remove = useMutation({
    mutationFn: (webhook: Webhook) => deleteWebhook(webhook.id),
    onSuccess: (_, webhook) => {
      setDeleting(null)
      showToast({ title: `${webhook.name} deleted` })
    },
    onSettled: refresh,
  })

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
        Send events to your own services as signed JSON. A webhook hears only what you can see, and turns itself off if you lose access.
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
            {list.data.webhooks.length ? (
              <RowList label="Webhooks">
                {list.data.webhooks.map((webhook) => (
                  <Row
                    actions={
                      <>
                        <Button onClick={() => setLog(webhook)} size="sm" variant="ghost">
                          Deliveries
                        </Button>
                        <Menu
                          items={[
                            {
                              label: 'Edit',
                              icon: 'pencil',
                              onSelect: () => setEditing(webhook),
                            },
                            {
                              label: 'Send a test',
                              icon: 'send',
                              disabled: webhook.state !== 'active',
                              onSelect: () => ping.mutate(webhook),
                            },
                            {
                              label: webhook.state === 'active' ? 'Pause' : 'Turn on',
                              icon: webhook.state === 'active' ? 'stop' : 'play',
                              onSelect: () => toggle.mutate(webhook),
                            },
                            {
                              label: 'Replace the secret',
                              icon: 'key',
                              onSelect: () => setReplacing(webhook),
                            },
                            { separator: true },
                            {
                              label: 'Delete',
                              icon: 'trash',
                              danger: true,
                              onSelect: () => setDeleting(webhook),
                            },
                          ]}
                          label={`${webhook.name} actions`}
                          trigger={<IconButton icon="more" label={`${webhook.name} actions`} size="sm" />}
                        />
                      </>
                    }
                    className={webhook.state === 'active' ? undefined : 'is-disabled'}
                    key={webhook.id}
                    leading={
                      <span className="aw-row__tile">
                        <Icon name="send" size={16} />
                      </span>
                    }
                    meta={
                      <span className="aw-row__metaline aw-wrap">
                        <code className="aw-inline-code">{webhook.url}</code>
                        <span>{webhook.projectName ?? 'Every project you can see'}</span>
                        <span>{webhook.eventTypes.join(', ')}</span>
                        <span title={webhook.lastDeliveryAt ? formatFullDate(webhook.lastDeliveryAt) : undefined}>
                          {webhook.lastDeliveryAt ? `Last sent ${formatRelative(webhook.lastDeliveryAt)}` : 'Nothing sent yet'}
                        </span>
                        {webhook.disabledReason && <span>{webhook.disabledReason}</span>}
                      </span>
                    }
                    title={
                      <span className="aw-row__titleline">
                        {webhook.name}
                        <StatusChip tone={STATE[webhook.state].tone}>{STATE[webhook.state].label}</StatusChip>
                      </span>
                    }
                  />
                ))}
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
            <p className="aw-quiet">{list.data.signing}</p>
          </Section>
        </>
      )}
      {editing && list.data && (
        <WebhookDialog
          list={list.data}
          onClose={() => setEditing(null)}
          onSaved={(result) => {
            setEditing(null)
            if (result.secret) setSecret({ name: result.name, value: result.secret })
            void refresh()
          }}
          webhook={editing === 'new' ? null : editing}
        />
      )}
      {log && <DeliveriesDialog onClose={() => setLog(null)} webhook={log} />}
      <ConfirmDialog
        busy={remove.isPending}
        confirmLabel="Delete webhook"
        error={remove.isError ? `The webhook wasn’t deleted. ${describeError(remove.error).message}` : null}
        onClose={() => {
          setDeleting(null)
          remove.reset()
        }}
        onConfirm={() => deleting && remove.mutate(deleting)}
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
        onConfirm={() => replacing && rotate.mutate(replacing)}
        open={replacing !== null}
        title={`Replace the secret for ${replacing?.name ?? 'this webhook'}?`}
      >
        Deliveries signed with the old secret will fail until the receiving service has the new one.
      </ConfirmDialog>
    </>
  )
}

function WebhookDialog({
  list,
  onClose,
  onSaved,
  webhook,
}: {
  list: WebhookList
  onClose: () => void
  onSaved: (result: { name: string; secret: string | null }) => void
  webhook: Webhook | null
}) {
  const [name, setName] = useState(webhook?.name ?? '')
  const [url, setUrl] = useState(webhook?.url ?? 'https://')
  const [projectId, setProjectId] = useState(webhook?.projectId ?? '')
  const [events, setEvents] = useState<string[]>(webhook?.eventTypes ?? ['run.finished'])
  const [error, setError] = useState<string | null>(null)
  const save = useMutation({
    mutationFn: async () => {
      if (webhook) {
        const saved = await updateWebhook(webhook, {
          name: name.trim(),
          url: url.trim(),
          eventTypes: events,
        })
        return { name: saved.name, secret: null }
      }
      const created = await createWebhook({
        name: name.trim(),
        url: url.trim(),
        eventTypes: events,
        projectId: projectId || null,
      })
      return { name: created.webhook.name, secret: created.secret }
    },
    onSuccess: onSaved,
    onError: (reason) => setError(`The webhook wasn’t saved. ${describeError(reason).message}`),
  })
  const submit = (event: FormEvent<HTMLFormElement>) => {
    event.preventDefault()
    if (!name.trim() || !url.trim() || !events.length) return
    save.mutate()
  }
  const dirty =
    name !== (webhook?.name ?? '') ||
    url !== (webhook?.url ?? 'https://') ||
    projectId !== (webhook?.projectId ?? '') ||
    events.join() !== (webhook?.eventTypes ?? ['run.finished']).join()

  return (
    <Dialog
      footer={
        <>
          <Button onClick={onClose}>Cancel</Button>
          <Button busy={save.isPending} disabled={!name.trim() || !events.length} type="submit" variant="primary">
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
        {!webhook && (
          <SelectField id="webhook-project" label="Hear from" onChange={(event) => setProjectId(event.target.value)} value={projectId}>
            <option value="">Every project you can see</option>
            {list.projects.map((project) => (
              <option key={project.id} value={project.id}>
                {project.name}
              </option>
            ))}
          </SelectField>
        )}
        <fieldset className="aw-fieldset">
          <legend className="aw-field__label">Events</legend>
          {list.eventTypes.map((type) => (
            <Checkbox
              checked={events.includes(type.name)}
              description={type.description}
              key={type.name}
              label={<code className="aw-inline-code">{type.name}</code>}
              onChange={(checked) =>
                setEvents((current) => (checked ? [...current, type.name] : current.filter((name) => name !== type.name)))
              }
            />
          ))}
        </fieldset>
        {error && <Banner tone="danger">{error}</Banner>}
      </div>
    </Dialog>
  )
}

function DeliveriesDialog({ onClose, webhook }: { onClose: () => void; webhook: Webhook }) {
  const queryClient = useQueryClient()
  const deliveries = useQuery({
    queryKey: automationKeys.deliveries(webhook.id),
    queryFn: () => listDeliveries(webhook.id),
  })
  const again = useMutation({
    mutationFn: (delivery: WebhookDelivery) => redeliver(webhook.id, delivery.id),
    onSuccess: () =>
      showToast({
        title: 'Sending it again',
        description: 'It keeps its delivery id, so the receiver can tell.',
      }),
    onError: (error) => toastError('It wasn’t sent again', error),
    onSettled: () =>
      queryClient.invalidateQueries({
        queryKey: automationKeys.deliveries(webhook.id),
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
          {deliveries.data.deliveries.map((delivery) => (
            <Row
              actions={
                delivery.state === 'pending' ? null : (
                  <Button disabled={again.isPending} onClick={() => again.mutate(delivery)} size="sm" variant="ghost">
                    Send again
                  </Button>
                )
              }
              key={delivery.id}
              meta={
                <span className="aw-row__metaline aw-wrap">
                  <span>
                    Delivery <code className="aw-inline-code">{delivery.id}</code>
                  </span>
                  <span title={formatFullDate(delivery.createdAt)}>{formatRelative(delivery.createdAt)}</span>
                  <span>{countLabel(delivery.attempts, 'attempt')}</span>
                  {delivery.lastStatusCode !== null && <span>HTTP {delivery.lastStatusCode}</span>}
                  {delivery.lastError && <span>{delivery.lastError}</span>}
                </span>
              }
              title={
                <span className="aw-row__titleline">
                  <code className="aw-inline-code">{delivery.eventType}</code>
                  <StatusChip tone={DELIVERY[delivery.state].tone}>{DELIVERY[delivery.state].label}</StatusChip>
                </span>
              }
            />
          ))}
        </RowList>
      ) : (
        <p className="aw-quiet">Nothing sent yet. Send a test to try it.</p>
      )}
    </Dialog>
  )
}
