import { useQuery, useQueryClient } from '@tanstack/react-query'
import { useState, type FormEvent, type ReactNode } from 'react'

import { callOp, invalidateOps, opKey, useOp, type OpInput, type OpOutput } from '../../api/ops'
import type { OpName } from '../../api/generated/operations'
import { refreshLayouts } from '../../experience/api'
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
  Row,
  RowList,
  Section,
  StatusChip,
  TextField,
  describeError,
  showToast,
} from '../../ui'

import './modules.css'

// M1.12: Settings › Modules › Installed › Connected. Outside apps installed
// as modules from their manifest's address: see what one asks for before it
// is connected, approve a manifest that asks for more, check one again, and
// disconnect it. Agents do the same through the connected_module_* tools.

type Connected = OpOutput<'connected_module_list'>['modules'][number]
type Manifest = Pick<Connected, 'name' | 'description' | 'publisher' | 'version' | 'digest' | 'scopes' | 'kinds' | 'operations' | 'events' | 'panels' | 'frameOrigins'>
type FileCheck = { checkedAt?: string; problems?: { path: string; message: string }[] }

const LIST = ['connected_module_list'] as const satisfies readonly OpName[]

function read<N extends OpName>(name: N, input: OpInput<N>, workspaceId: string) {
  return { queryKey: opKey(name, input, workspaceId), queryFn: (): Promise<OpOutput<N>> => callOp(name, input, { workspaceId }) }
}

function plural(count: number, one: string, many = `${one}s`) {
  return `${count} ${count === 1 ? one : many}`
}

function host(address: string) {
  try {
    return new URL(address).host
  } catch {
    return address
  }
}

/**
 * The scopes ticked to start with: of the ones this server has, what is
 * approved already (a preview marks all it would approve), or every one.
 */
function startingScopes(manifest: Manifest, everything = false) {
  return new Set(manifest.scopes.filter((scope) => scope.known && (everything || scope.required || scope.approved)).map((scope) => scope.scope))
}

export function ConnectedModules({ workspaceId }: { workspaceId: string }) {
  const queryClient = useQueryClient()
  const list = useQuery(read('connected_module_list', {}, workspaceId))
  const [connecting, setConnecting] = useState(false)
  const [reviewing, setReviewing] = useState<Connected | null>(null)
  const [revoking, setRevoking] = useState<Connected | null>(null)
  const settle = () => {
    void refreshLayouts(queryClient)
    void invalidateOps(queryClient, [...LIST], workspaceId)
  }
  const refresh = useOp('connected_module_refresh', {
    workspaceId,
    invalidates: [...LIST],
    onSuccess: ({ module, refreshed }) => {
      if (refreshed === 'pending') {
        showToast({ tone: 'info', title: `${module.name} asks for more`, description: 'Review the changes to approve them.' })
      } else if (refreshed === 'applied') {
        showToast({ tone: 'success', title: `${module.name} is up to date`, description: `Now version ${module.version}.` })
      } else {
        showToast({ tone: 'success', title: `${module.name} hasn’t changed` })
      }
    },
  })
  const test = useOp('connected_module_test', {
    workspaceId,
    invalidates: [...LIST],
    onSuccess: ({ problems }) =>
      showToast(
        problems.length
          ? { tone: 'danger', title: `Its files have ${plural(problems.length, 'problem')}`, description: problems[0].message }
          : { tone: 'success', title: 'Its files pass every check' },
      ),
  })
  const revoke = useOp('connected_module_revoke', {
    workspaceId,
    invalidates: [...LIST],
    onSuccess: (module) => {
      setRevoking(null)
      settle()
      showToast({ tone: 'success', title: `${module.name} is disconnected` })
    },
  })

  const add = (
    <Button icon="plus" onClick={() => setConnecting(true)} size="sm">
      Connect a module
    </Button>
  )
  const section = (body: ReactNode, actions?: ReactNode) => (
    <Section
      actions={actions}
      description="Outside apps installed as modules. Each acts through its own account here, holding only the scopes you approved."
      title="Connected"
    >
      {body}
    </Section>
  )

  let body: ReactNode
  if (list.isPending) body = section(<ListSkeleton label="Loading connected modules" rows={2} />)
  else if (list.isError) body = section(<ErrorState error={list.error} onRetry={() => void list.refetch()} title="Connected modules didn’t load" />)
  else if (list.data.modules.length === 0)
    body = section(
      <EmptyState action={add} icon="link" title="Nothing connected yet">
        Connect an outside app by its manifest’s address. You see what it asks for before anything is installed.
      </EmptyState>,
    )
  else
    body = section(
      <RowList>
        {list.data.modules.map((module) => {
          const check = (module.fileCheck ?? null) as FileCheck | null
          const problems = check?.problems?.length ?? 0
          return (
            <Row
              actions={
                <span className="aw-row__metaline">
                  {module.pending ? (
                    <Button onClick={() => setReviewing(module)} size="sm">
                      Review changes
                    </Button>
                  ) : module.state === 'pending' ? (
                    <StatusChip tone="warning">Waiting for the app</StatusChip>
                  ) : problems ? (
                    <StatusChip tone="danger">{plural(problems, 'file problem')}</StatusChip>
                  ) : null}
                  <Menu
                    items={[
                      ...(module.pending
                        ? [{ label: 'Review changes…', icon: 'checklist' as const, onSelect: () => window.requestAnimationFrame(() => setReviewing(module)) }]
                        : []),
                      { label: 'Check for changes', icon: 'retry' as const, onSelect: () => refresh.mutate({ moduleId: module.moduleId }) },
                      ...(check ? [{ label: 'Check its files', icon: 'check' as const, onSelect: () => test.mutate({ moduleId: module.moduleId }) }] : []),
                      { separator: true as const },
                      {
                        label: 'Disconnect…',
                        icon: 'unlink' as const,
                        danger: true,
                        onSelect: () => window.requestAnimationFrame(() => setRevoking(module)),
                      },
                    ]}
                    label={`${module.name} actions`}
                    trigger={<IconButton icon="more" label={`${module.name} actions`} size="sm" />}
                  />
                </span>
              }
              key={module.moduleId}
              leading={<Icon name="link" size={18} />}
              meta={[module.description, module.publisher ? `By ${module.publisher}` : null, `Version ${module.version}`, host(module.baseUrl)]
                .filter(Boolean)
                .join(' · ')}
              title={module.name}
            />
          )
        })}
      </RowList>,
      add,
    )

  return (
    <>
      {body}
      {connecting ? (
        <ConnectDialog
          onClose={() => setConnecting(false)}
          onConnected={() => {
            setConnecting(false)
            settle()
          }}
          workspaceId={workspaceId}
        />
      ) : null}
      {reviewing?.pending ? (
        <ReviewDialog
          module={reviewing}
          onClose={() => setReviewing(null)}
          onApproved={() => {
            setReviewing(null)
            settle()
          }}
          pending={reviewing.pending}
          workspaceId={workspaceId}
        />
      ) : null}
      <ConfirmDialog
        busy={revoke.isPending}
        confirmLabel="Disconnect"
        error={revoke.isError ? describeError(revoke.error).message : undefined}
        onClose={() => {
          setRevoking(null)
          revoke.reset()
        }}
        onConfirm={() => revoking && revoke.mutate({ moduleId: revoking.moduleId })}
        open={revoking !== null}
        title={`Disconnect ${revoking?.name ?? 'the module'}?`}
        tone="danger"
      >
        Its account is turned off and its credentials stop working at once. What it already made stays. To use it again, connect it again.
      </ConfirmDialog>
    </>
  )
}

/** What a manifest adds and asks for, with a box for each scope it asks for. */
function ManifestDetails({
  chosen,
  manifest,
  onChoose,
}: {
  chosen: Set<string>
  manifest: Manifest
  onChoose: (scope: string, on: boolean) => void
}) {
  const adds = [
    manifest.kinds.length ? plural(manifest.kinds.length, 'kind') : null,
    manifest.operations.length ? plural(manifest.operations.length, 'operation') : null,
    manifest.panels.length ? plural(manifest.panels.length, 'panel') : null,
  ].filter(Boolean)
  return (
    <>
      <div className="aw-connect__about">
        <span className="aw-quiet">
          {[manifest.publisher ? `By ${manifest.publisher}` : null, `Version ${manifest.version}`].filter(Boolean).join(' · ')}
        </span>
        {manifest.description ? <p>{manifest.description}</p> : null}
      </div>
      <fieldset className="aw-builder__parts">
        <legend className="aw-field__label">What it can do here</legend>
        {manifest.scopes.length === 0 ? (
          <p className="aw-quiet">It asks for nothing beyond its own account.</p>
        ) : (
          <ul className="aw-builder__list">
            {manifest.scopes.map((scope) => (
              <li className="aw-builder__part" key={scope.scope}>
                <Checkbox
                  checked={chosen.has(scope.scope)}
                  description={
                    !scope.known
                      ? 'This server doesn’t have this, so it can’t be given.'
                      : scope.required
                        ? `${scope.why} It needs this to work.`
                        : scope.why
                  }
                  disabled={!scope.known || scope.required}
                  label={scope.known ? scope.label : scope.scope}
                  onChange={(on) => onChoose(scope.scope, on)}
                />
              </li>
            ))}
          </ul>
        )}
      </fieldset>
      <dl className="aw-connect__facts">
        {adds.length ? (
          <>
            <dt>Adds</dt>
            <dd>{adds.join(', ')}</dd>
          </>
        ) : null}
        {manifest.events.length ? (
          <>
            <dt>Hears about</dt>
            <dd>{manifest.events.join(', ')}</dd>
          </>
        ) : null}
        {manifest.frameOrigins.length ? (
          <>
            <dt>Shows pages from</dt>
            <dd>{manifest.frameOrigins.map(host).join(', ')}</dd>
          </>
        ) : null}
      </dl>
    </>
  )
}

function useScopeChoice(manifest: Manifest | null) {
  const [chosen, setChosen] = useState<Set<string>>(() => (manifest ? startingScopes(manifest, true) : new Set()))
  const choose = (scope: string, on: boolean) =>
    setChosen((current) => {
      const next = new Set(current)
      if (on) next.add(scope)
      else next.delete(scope)
      return next
    })
  return { chosen, setChosen, choose }
}

/** Connect an outside app: its address, then what it asks for, then install. */
function ConnectDialog({ onClose, onConnected, workspaceId }: { onClose: () => void; onConnected: () => void; workspaceId: string }) {
  const [url, setUrl] = useState('')
  const [preview, setPreview] = useState<Connected | null>(null)
  const [busy, setBusy] = useState(false)
  const [error, setError] = useState<string | null>(null)
  const { chosen, setChosen, choose } = useScopeChoice(null)

  const submit = async (event: FormEvent<HTMLFormElement>) => {
    event.preventDefault()
    const address = url.trim()
    if (!address) {
      setError('Paste the address of the app’s manifest.')
      return
    }
    setBusy(true)
    setError(null)
    try {
      if (!preview) {
        const shown = await callOp('connected_module_install', { url: address }, { workspaceId, preview: true })
        setPreview(shown)
        setChosen(startingScopes(shown))
      } else {
        const module = await callOp('connected_module_install', { url: address, digest: preview.digest, scopes: [...chosen] }, { workspaceId })
        showToast({
          tone: 'success',
          title: `${module.name} is connected`,
          description: module.state === 'pending' ? 'It finishes setting up as soon as the app answers.' : undefined,
        })
        onConnected()
      }
    } catch (failure) {
      const described = describeError(failure)
      if (preview && (failure as { code?: string }).code === 'conflict') {
        // The manifest changed since the preview: show the new one.
        setPreview(null)
      }
      setError(described.message)
    } finally {
      setBusy(false)
    }
  }

  return (
    <Dialog
      description={
        preview
          ? 'Choose what it may do here. What it needs to work stays on, and nothing is installed until you connect it.'
          : 'Paste the address of the app’s manifest. You see who publishes it and what it asks for before anything is installed.'
      }
      dirty={url.trim().length > 0}
      footer={
        <>
          <Button onClick={onClose} variant="ghost">
            Cancel
          </Button>
          {preview ? (
            <Button onClick={() => setPreview(null)} variant="ghost">
              Back
            </Button>
          ) : null}
          <Button busy={busy} type="submit" variant="primary">
            {preview ? 'Connect' : 'Look it up'}
          </Button>
        </>
      }
      onClose={onClose}
      onSubmit={(event) => void submit(event)}
      open
      title={preview ? `Connect ${preview.name}?` : 'Connect a module'}
      width={560}
    >
      {error ? <Banner tone="danger">{error}</Banner> : null}
      {preview ? (
        <ManifestDetails chosen={chosen} manifest={preview} onChoose={choose} />
      ) : (
        <TextField
          autoFocus
          inputMode="url"
          label="Manifest address"
          maxLength={2048}
          mono
          onChange={(event) => setUrl(event.target.value)}
          placeholder="https://"
          required
          type="url"
          value={url}
        />
      )}
    </Dialog>
  )
}

/** A re-read manifest that asks for more: approve it, or leave the module as it is. */
function ReviewDialog({
  module,
  onApproved,
  onClose,
  pending,
  workspaceId,
}: {
  module: Connected
  onApproved: () => void
  onClose: () => void
  pending: NonNullable<Connected['pending']>
  workspaceId: string
}) {
  const { chosen, choose } = useScopeChoice(pending)
  const approve = useOp('connected_module_approve', {
    workspaceId,
    invalidates: [...LIST],
    onSuccess: (approved) => {
      showToast({ tone: 'success', title: `${approved.name} is up to date`, description: `Now version ${approved.version}.` })
      onApproved()
    },
  })
  const had = new Set(module.scopes.filter((scope) => scope.approved).map((scope) => scope.scope))
  const added = pending.scopes.filter((scope) => scope.known && !had.has(scope.scope))
  return (
    <Dialog
      description={`${module.name} changed its manifest${pending.version !== module.version ? ` to version ${pending.version}` : ''} and asks for more than you approved. Until you approve, it keeps working as it did.`}
      footer={
        <>
          <Button onClick={onClose} variant="ghost">
            Not now
          </Button>
          <Button busy={approve.isPending} type="submit" variant="primary">
            Approve
          </Button>
        </>
      }
      onClose={onClose}
      onSubmit={(event) => {
        event.preventDefault()
        approve.mutate({ moduleId: module.moduleId, digest: pending.digest, scopes: [...chosen] })
      }}
      open
      title={`Review changes to ${module.name}`}
      width={560}
    >
      {approve.isError ? <Banner tone="danger">{describeError(approve.error).message}</Banner> : null}
      {added.length ? (
        <Banner tone="warning">
          New: {added.map((scope) => scope.label).join(', ')}.
        </Banner>
      ) : null}
      <ManifestDetails chosen={chosen} manifest={pending} onChoose={choose} />
    </Dialog>
  )
}
