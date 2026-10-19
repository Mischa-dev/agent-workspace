import { useMutation, useQuery, useQueryClient } from '@tanstack/react-query'
import { useState } from 'react'

import { describeError, toastError } from '../../app/mutations'
import {
  Banner,
  Button,
  ConfirmDialog,
  EmptyState,
  ErrorState,
  PageHeader,
  Row,
  RowList,
  Section,
  SettingsSkeleton,
  StatusChip,
  Switch,
  countLabel,
  showToast,
} from '../../ui'
import type { SettingsPageProps } from '../types'
import { CommandsOffNote, HarnessForm } from './HarnessForm'
import { harnessKeys, listHarnesses, removeHarness, updateHarness, type CustomHarness, type HarnessList } from './harnessesApi'
import './harnesses.css'

/** Workspace › Harnesses: your own agents, as a command on this server or a connected runner. */
export default function HarnessesPage({ workspace }: SettingsPageProps) {
  const organizationId = workspace?.id ?? ''
  const list = useQuery({ queryKey: harnessKeys.list(organizationId), queryFn: () => listHarnesses(organizationId), enabled: Boolean(organizationId) })
  const [editing, setEditing] = useState<CustomHarness | 'new' | null>(null)
  const [removing, setRemoving] = useState<CustomHarness | null>(null)
  if (!workspace) return null
  const data = list.data

  return (
    <>
      <PageHeader
        actions={
          data?.canManage ? (
            <Button icon="plus" onClick={() => setEditing('new')} variant="primary">
              Register a harness
            </Button>
          ) : null
        }
        icon="terminal"
        title="Harnesses"
      >
        Bring your own agent. Registered harnesses show up in the Launch dialog and on project agents, with the models their manifest lists.
      </PageHeader>
      {list.isPending ? <SettingsSkeleton label="Loading harnesses" /> : null}
      {list.isError ? <ErrorState error={list.error} onRetry={() => void list.refetch()} title="Harnesses didn’t load" /> : null}
      {data && !data.commandsAllowed ? <CommandsOffNote /> : null}
      {data && !data.canManage ? <Banner tone="info">Only workspace owners and admins can register or change harnesses.</Banner> : null}
      {data ? (
        <Section description="A command runs on this server for each turn. A connected runner is an agent that does the work wherever it runs." title="Registered">
          {data.harnesses.length === 0 ? (
            <EmptyState
              action={data.canManage ? <Button onClick={() => setEditing('new')}>Register a harness</Button> : undefined}
              icon="terminal"
              title="No harnesses registered"
            >
              The built-in harnesses are always there. Register your own to launch it on tasks.
            </EmptyState>
          ) : (
            <RowList label="Registered harnesses">
              {data.harnesses.map((harness) => (
                <HarnessRow
                  canManage={data.canManage}
                  harness={harness}
                  key={harness.id}
                  onEdit={() => setEditing(harness)}
                  onRemove={() => setRemoving(harness)}
                  organizationId={organizationId}
                />
              ))}
            </RowList>
          )}
        </Section>
      ) : null}
      {editing && data ? (
        <HarnessForm harness={editing === 'new' ? null : editing} list={data} onClose={() => setEditing(null)} organizationId={organizationId} />
      ) : null}
      {removing && data ? <RemoveDialog harness={removing} list={data} onClose={() => setRemoving(null)} organizationId={organizationId} /> : null}
    </>
  )
}

function HarnessRow({
  canManage,
  harness,
  onEdit,
  onRemove,
  organizationId,
}: {
  canManage: boolean
  harness: CustomHarness
  onEdit: () => void
  onRemove: () => void
  organizationId: string
}) {
  const client = useQueryClient()
  const toggle = useMutation({
    mutationFn: (enabled: boolean) => updateHarness(organizationId, harness.id, { version: harness.version, enabled }),
    onSuccess: (saved) => {
      void client.invalidateQueries({ queryKey: harnessKeys.list(organizationId) })
      showToast({
        tone: 'success',
        title: saved.enabled ? `${saved.name} turned on` : `${saved.name} turned off`,
        onUndo: () =>
          updateHarness(organizationId, saved.id, { version: saved.version, enabled: !saved.enabled }).then(
            () => client.invalidateQueries({ queryKey: harnessKeys.list(organizationId) }),
            (reason) => toastError(`${saved.name} wasn’t changed back`, reason),
          ),
      })
    },
    onError: (error) => toastError(`${harness.name} wasn’t changed`, error),
  })
  const models = harness.resolved?.defaultModels?.length ?? 0
  return (
    <Row
      actions={
        canManage ? (
          <div className="aw-harnesses__actions">
            <Switch checked={harness.enabled} disabled={toggle.isPending} label="Enabled" onChange={(enabled) => toggle.mutate(enabled)} />
            <Button icon="pencil" onClick={onEdit} size="sm" variant="ghost">
              Edit
            </Button>
            <Button icon="trash" onClick={onRemove} size="sm" variant="ghost">
              Remove
            </Button>
          </div>
        ) : (
          <StatusChip tone={harness.enabled ? 'success' : 'muted'}>{harness.enabled ? 'Enabled' : 'Turned off'}</StatusChip>
        )
      }
      meta={
        <span className="aw-harnesses__meta">
          <span>{harness.transport === 'runner' ? `Hands off to ${harness.runnerName ?? 'a runner'}` : 'Command on this server'}</span>
          <span>{countLabel(models, 'model')}</span>
          {harness.resolved ? null : <span>The manifest no longer loads</span>}
        </span>
      }
      title={harness.name}
    >
      {harness.resolved?.description ? <p className="aw-row__meta">{harness.resolved.description}</p> : null}
    </Row>
  )
}

function RemoveDialog({ harness, list, onClose, organizationId }: { harness: CustomHarness; list: HarnessList; onClose: () => void; organizationId: string }) {
  const client = useQueryClient()
  const remove = useMutation({
    mutationFn: () => removeHarness(organizationId, harness.id),
    onSuccess: () => {
      client.setQueryData<HarnessList>(harnessKeys.list(organizationId), { ...list, harnesses: list.harnesses.filter((item) => item.id !== harness.id) })
      void client.invalidateQueries({ queryKey: harnessKeys.list(organizationId) })
      showToast({ tone: 'success', title: `${harness.name} removed` })
      onClose()
    },
  })
  return (
    <ConfirmDialog
      busy={remove.isPending}
      confirmLabel="Remove harness"
      error={remove.isError ? `It wasn’t removed. ${describeError(remove.error).message}` : null}
      onClose={onClose}
      onConfirm={() => remove.mutate()}
      open
      title={`Remove ${harness.name}?`}
    >
      New runs can’t pick it any more, and runs it already did keep their history. While accounts still use it, turn it off
      instead.
    </ConfirmDialog>
  )
}
