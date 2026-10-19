import { useState, type FormEvent } from 'react'
import { useMutation, useQuery, useQueryClient } from '@tanstack/react-query'

import { describeError } from '../../app/mutations'
import {
  Banner,
  Button,
  ConfirmDialog,
  Dialog,
  EmptyState,
  ErrorState,
  Icon,
  ListSkeleton,
  PageHeader,
  Row,
  RowList,
  Section,
  TextField,
  formatFullDate,
  formatRelative,
  formatShortDate,
  showToast,
} from '../../ui'
import type { SettingsPageProps } from '../types'
import {
  deleteWorkspaceSecret,
  listWorkspaceSecrets,
  putWorkspaceSecret,
  workspaceKeys,
  type WorkspaceSecret,
} from './api'

const NAME_PATTERN = /^[a-z][a-z0-9_.:-]{0,119}$/

/** Workspace › Keys: API keys and other secrets the server uses for the workspace. Write-only. */
export default function KeysPage({ workspace }: SettingsPageProps) {
  const organizationId = workspace?.id ?? ''
  const queryClient = useQueryClient()
  const secrets = useQuery({
    queryKey: workspaceKeys.secrets(organizationId),
    queryFn: () => listWorkspaceSecrets(organizationId),
    enabled: Boolean(organizationId),
  })
  const [editing, setEditing] = useState<WorkspaceSecret | 'new' | null>(null)
  const [deleting, setDeleting] = useState<WorkspaceSecret | null>(null)
  const refresh = () => queryClient.invalidateQueries({ queryKey: workspaceKeys.secrets(organizationId) })
  const remove = useMutation({
    mutationFn: (secret: WorkspaceSecret) => deleteWorkspaceSecret(organizationId, secret.name),
    onSuccess: (_result, secret) => {
      setDeleting(null)
      showToast({ tone: 'success', title: `${secret.name} deleted` })
    },
    onSettled: refresh,
  })
  if (!workspace) return null

  return (
    <>
      <PageHeader
        actions={
          secrets.data?.configured ? (
            <Button icon="plus" onClick={() => setEditing('new')} variant="primary">
              Add key
            </Button>
          ) : null
        }
        icon="lock"
        title="Keys"
      >
        API keys and other secrets the server uses for {workspace.name}. Once saved, a key can be replaced or deleted but
        never shown again.
      </PageHeader>
      {secrets.isPending ? (
        <ListSkeleton label="Loading keys" rows={2} />
      ) : secrets.isError ? (
        <ErrorState error={secrets.error} onRetry={() => void secrets.refetch()} title="Keys didn’t load" />
      ) : (
        <Section id="workspace-keys-heading" title="Workspace keys">
          {!secrets.data.configured && (
            <Banner role="note" tone="info" title="Keys can’t be saved on this server yet.">
              Whoever runs this server has to give it an encryption key first.
            </Banner>
          )}
          {secrets.data.secrets.length ? (
            <RowList label="Workspace keys">
              {secrets.data.secrets.map((secret) => (
                <Row
                  actions={
                    <>
                      {secrets.data.configured && (
                        <Button onClick={() => setEditing(secret)} size="sm" variant="ghost">
                          Replace
                        </Button>
                      )}
                      <Button icon="trash" onClick={() => setDeleting(secret)} size="sm" variant="ghost">
                        Delete
                      </Button>
                    </>
                  }
                  key={secret.id}
                  leading={
                    <span className="aw-row__tile">
                      <Icon name="lock" size={16} />
                    </span>
                  }
                  meta={
                    <span className="aw-row__metaline aw-wrap">
                      {secret.lastFour ? <code className="aw-inline-code">••••{secret.lastFour}</code> : <span>Short value</span>}
                      {secret.description && <span>{secret.description}</span>}
                      <span title={formatFullDate(secret.updatedAt)}>
                        Updated {formatShortDate(secret.updatedAt)}
                        {secret.createdByName ? ` by ${secret.createdByName}` : ''}
                      </span>
                      {secret.lastUsedAt ? (
                        <span title={formatFullDate(secret.lastUsedAt)}>Used {formatRelative(secret.lastUsedAt)}</span>
                      ) : (
                        <span>Not used yet</span>
                      )}
                    </span>
                  }
                  title={<code className="aw-setting__key">{secret.name}</code>}
                />
              ))}
            </RowList>
          ) : (
            <EmptyState
              action={
                secrets.data.configured ? (
                  <Button icon="plus" onClick={() => setEditing('new')}>
                    Add a key
                  </Button>
                ) : null
              }
              icon="lock"
              title="No keys yet"
            >
              {secrets.data.configured ? 'Add one for the providers and services this workspace uses.' : null}
            </EmptyState>
          )}
        </Section>
      )}
      <SecretDialog
        existing={editing === 'new' ? null : editing}
        onClose={() => setEditing(null)}
        onSaved={(secret) => {
          setEditing(null)
          showToast({ tone: 'success', title: `${secret.name} saved` })
          void refresh()
        }}
        open={editing !== null}
        organizationId={organizationId}
        taken={(secrets.data?.secrets ?? []).map((secret) => secret.name)}
      />
      <ConfirmDialog
        busy={remove.isPending}
        confirmLabel="Delete key"
        error={remove.isError ? `The key wasn’t deleted. ${describeError(remove.error).message}` : null}
        onClose={() => {
          setDeleting(null)
          remove.reset()
        }}
        onConfirm={() => deleting && remove.mutate(deleting)}
        open={deleting !== null}
        title={`Delete ${deleting?.name ?? 'this key'}?`}
      >
        Anything that uses it stops working until someone adds it again. This can’t be undone.
      </ConfirmDialog>
    </>
  )
}

function SecretDialog({
  existing,
  onClose,
  onSaved,
  open,
  organizationId,
  taken,
}: {
  existing: WorkspaceSecret | null
  onClose: () => void
  onSaved: (secret: WorkspaceSecret) => void
  open: boolean
  organizationId: string
  taken: string[]
}) {
  const [name, setName] = useState('')
  const [value, setValue] = useState('')
  const [description, setDescription] = useState('')
  const [error, setError] = useState<string | null>(null)
  const [shownFor, setShownFor] = useState<string | null>(null)
  // Start fresh each time it opens, with the key's name and description when replacing.
  const opening = open ? (existing?.id ?? 'new') : null
  if (opening !== shownFor) {
    setShownFor(opening)
    setName(existing?.name ?? '')
    setDescription(existing?.description ?? '')
    setValue('')
    setError(null)
  }
  const keyName = existing ? existing.name : name.trim()
  const save = useMutation({
    mutationFn: () => putWorkspaceSecret(organizationId, keyName, value, description.trim() || null),
    onSuccess: (secret) => {
      setValue('')
      onSaved(secret)
    },
    onError: (reason) => setError(`The key wasn’t saved. ${describeError(reason).message}`),
  })
  const nameError =
    !existing && keyName && !NAME_PATTERN.test(keyName)
      ? 'Start with a lowercase letter; use lowercase letters, digits and _ . : - only.'
      : !existing && taken.includes(keyName)
        ? 'There’s already a key with this name. Replace it instead.'
        : null
  const submit = (event: FormEvent<HTMLFormElement>) => {
    event.preventDefault()
    if (!keyName || nameError || !value) return
    save.mutate()
  }
  return (
    <Dialog
      description={
        existing
          ? 'The new value takes effect at once. The old one can’t be recovered.'
          : 'Paste the value once. It’s encrypted on the server and never shown again.'
      }
      footer={
        <>
          <Button onClick={onClose}>Cancel</Button>
          <Button busy={save.isPending} disabled={!keyName || Boolean(nameError) || !value} type="submit" variant="primary">
            {existing ? 'Replace key' : 'Save key'}
          </Button>
        </>
      }
      dirty={Boolean(value || (!existing && name))}
      onClose={onClose}
      onSubmit={submit}
      open={open}
      title={existing ? `Replace ${existing.name}` : 'Add a key'}
    >
      <div className="aw-form">
        {!existing && (
          <TextField
            autoComplete="off"
            error={nameError}
            hint="Such as search_api_key."
            id="secret-name"
            label="Name"
            maxLength={120}
            mono
            onChange={(event) => setName(event.target.value.toLowerCase())}
            spellCheck={false}
            value={name}
          />
        )}
        <TextField
          autoComplete="new-password"
          id="secret-value"
          label="Value"
          onChange={(event) => setValue(event.target.value)}
          spellCheck={false}
          type="password"
          value={value}
        />
        <TextField
          id="secret-description"
          label="Description (optional)"
          maxLength={200}
          onChange={(event) => setDescription(event.target.value)}
          value={description}
        />
        {error && <Banner tone="danger">{error}</Banner>}
      </div>
    </Dialog>
  )
}
