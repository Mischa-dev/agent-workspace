import { useState, type FormEvent } from 'react'
import { useQuery } from '@tanstack/react-query'

import { toastError } from '../app/mutations'
import { opQuery, useOp } from '../api/ops'
import type { HookView } from '../api/generated/operations'
import { OneTimeValue } from '../settings/OneTimeValue'
import {
  Button,
  ConfirmDialog,
  Dialog,
  ErrorState,
  Icon,
  IconButton,
  ListSkeleton,
  Row,
  RowList,
  Section,
  TextField,
  formatFullDate,
  formatRelative,
  showToast,
} from '../ui'

/** What a refused delivery's reason means, in words. */
const REFUSALS: Record<string, string> = {
  no_delivery_id: 'no delivery id',
  stale_timestamp: 'too old or from the future',
  bad_signature: 'signature didn’t match',
  too_large: 'too large',
}

function describeRecent(hook: HookView) {
  const last = hook.recent[0]
  if (!last) return 'No deliveries yet'
  if (last.outcome === 'accepted') return `Last delivery ${formatRelative(last.receivedAt)}`
  return `Last delivery refused ${formatRelative(last.receivedAt)}: ${REFUSALS[last.reason ?? ''] ?? last.reason ?? 'refused'}`
}

/**
 * A space's incoming hooks (K3.26): addresses an outside service posts to,
 * which start rules with the "a delivery comes in" trigger. The secret
 * shows once, when the hook is made.
 */
export function IncomingHooksSection({ manage, projectId }: { manage: boolean; projectId: string }) {
  const hooks = useQuery(opQuery('incoming_hook_list', { spaceId: projectId }))
  const [making, setMaking] = useState(false)
  const [made, setMade] = useState<{ name: string; url: string; secret: string } | null>(null)
  const [deleting, setDeleting] = useState<HookView | null>(null)
  const remove = useOp('incoming_hook_delete', {
    invalidates: ['incoming_hook_list'],
    onSuccess: () => {
      setDeleting(null)
      showToast({ title: 'Hook deleted' })
    },
  })

  return (
    <Section
      actions={
        manage ? (
          <Button icon="plus" onClick={() => setMaking(true)} size="sm">
            New hook
          </Button>
        ) : null
      }
      description="Addresses another service can post to, to start rules here. Each delivery is signed with the hook’s secret."
      id="incoming-hooks-heading"
      title="Incoming hooks"
    >
      {made && (
        <OneTimeValue copyLabel="Copy secret" onDone={() => setMade(null)} title={`${made.name} is ready`} value={made.secret}>
          Post deliveries to <code>{made.url}</code>, signed with this secret. It won’t be shown again: if it’s lost, delete the hook and make
          another.
        </OneTimeValue>
      )}
      {hooks.isPending ? (
        <ListSkeleton label="Loading incoming hooks" rows={1} />
      ) : hooks.isError ? (
        <ErrorState error={hooks.error} onRetry={() => void hooks.refetch()} title="Incoming hooks didn’t load" />
      ) : hooks.data.hooks.length ? (
        <RowList label="Incoming hooks">
          {hooks.data.hooks.map((hook) => (
            <Row
              actions={
                manage ? <IconButton icon="trash" label={`Delete ${hook.name}`} onClick={() => setDeleting(hook)} size="sm" /> : null
              }
              key={hook.id}
              leading={
                <span className="aw-row__tile">
                  <Icon name="link" size={16} />
                </span>
              }
              meta={
                <span className="aw-row__metaline aw-wrap">
                  <code>{hook.url}</code>
                  <span title={hook.recent[0] ? formatFullDate(hook.recent[0].receivedAt) : undefined}>{describeRecent(hook)}</span>
                </span>
              }
              title={hook.name}
            />
          ))}
        </RowList>
      ) : (
        <p className="aw-quiet">No incoming hooks yet.</p>
      )}
      {making && (
        <NewHookDialog
          onClose={() => setMaking(false)}
          onMade={(value) => {
            setMaking(false)
            setMade(value)
          }}
          projectId={projectId}
        />
      )}
      <ConfirmDialog
        busy={remove.isPending}
        confirmLabel="Delete hook"
        onClose={() => setDeleting(null)}
        onConfirm={() =>
          deleting &&
          remove.mutate(
            { spaceId: projectId, hookId: deleting.id },
            {
              onError: (error) => {
                setDeleting(null)
                toastError('The hook wasn’t deleted', error)
              },
            },
          )
        }
        open={deleting !== null}
        title={deleting ? `Delete ${deleting.name}?` : 'Delete this hook?'}
      >
        Its address stops taking deliveries at once, and rules started by it stop firing.
      </ConfirmDialog>
    </Section>
  )
}

function NewHookDialog({
  onClose,
  onMade,
  projectId,
}: {
  onClose: () => void
  onMade: (made: { name: string; url: string; secret: string }) => void
  projectId: string
}) {
  const [name, setName] = useState('')
  const create = useOp('incoming_hook_create', {
    invalidates: ['incoming_hook_list'],
    onSuccess: (value) => {
      if (value.secret) onMade({ name: value.hook.name, url: value.hook.url, secret: value.secret })
      else onClose()
    },
  })
  const submit = (event: FormEvent<HTMLFormElement>) => {
    event.preventDefault()
    if (!name.trim()) return
    create.mutate({ spaceId: projectId, name: name.trim() }, { onError: (error) => toastError('The hook wasn’t made', error) })
  }
  return (
    <Dialog
      dirty={name.trim() !== ''}
      footer={
        <>
          <Button onClick={onClose}>Cancel</Button>
          <Button busy={create.isPending} disabled={!name.trim()} type="submit" variant="primary">
            Make hook
          </Button>
        </>
      }
      onClose={onClose}
      onSubmit={submit}
      open
      title="New incoming hook"
      width={480}
    >
      <div className="aw-form">
        <TextField
          hint="What sends to it, in a few words."
          id="incoming-hook-name"
          label="Name"
          maxLength={120}
          onChange={(event) => setName(event.target.value)}
          placeholder="Billing alerts"
          required
          value={name}
        />
      </div>
    </Dialog>
  )
}
