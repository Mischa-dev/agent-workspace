import { useState, type FormEvent } from 'react'
import { useMutation, useQuery, useQueryClient } from '@tanstack/react-query'

import { describeError, toastError } from '../../app/mutations'
import {
  Banner,
  Button,
  Dialog,
  EmptyState,
  ErrorState,
  Icon,
  ListSkeleton,
  PageHeader,
  Row,
  RowList,
  Section,
  SegmentedControl,
  SelectField,
  StatusChip,
  TextField,
  formatRelative,
  formatShortDate,
  showToast,
  type ChipTone,
} from '../../ui'
import { OneTimeValue } from '../OneTimeValue'
import {
  accountKeys,
  createApiToken,
  listApiTokens,
  revokeApiToken,
  type ApiToken,
  type ApiTokenProject,
} from './api'

const EXPIRY_PRESETS = [7, 30, 90, 365]
const NAME_PATTERN = /^[A-Za-z0-9][A-Za-z0-9._ -]*$/
const STATUS: Record<ApiToken['status'], { label: string; tone: ChipTone }> = {
  active: { label: 'Active', tone: 'success' },
  revoked: { label: 'Revoked', tone: 'muted' },
  expired: { label: 'Expired', tone: 'muted' },
}

/** Account › API tokens: personal tokens for scripts and tools, one Project each, acting as you. */
export default function ApiTokensPage() {
  const queryClient = useQueryClient()
  const tokens = useQuery({ queryKey: accountKeys.apiTokens, queryFn: listApiTokens })
  const [created, setCreated] = useState<{ token: string; credential: ApiToken } | null>(null)
  const [revoking, setRevoking] = useState<ApiToken | null>(null)
  const revoke = useMutation({
    mutationFn: revokeApiToken,
    onSuccess: (token) => {
      setRevoking(null)
      showToast({ tone: 'success', title: `${token.name} revoked`, description: 'Anything using it stops working now.' })
    },
    onError: (error) => {
      setRevoking(null)
      toastError('Couldn’t revoke the token', error)
    },
    onSettled: () => queryClient.invalidateQueries({ queryKey: accountKeys.apiTokens }),
  })

  return (
    <>
      <PageHeader icon="key" title="API tokens">
        Tokens let scripts and tools reach one Project as you, through the MCP server. Treat them like passwords.
      </PageHeader>
      {created && (
        <OneTimeValue
          onDone={() => setCreated(null)}
          title={`${created.credential.name} is ready`}
          value={created.token}
        >
          Copy it now and keep it somewhere safe. It can’t be shown again; if you lose it, revoke it and create another.
        </OneTimeValue>
      )}
      {tokens.isPending ? (
        <ListSkeleton label="Loading API tokens" rows={2} />
      ) : tokens.isError ? (
        <ErrorState action={<Button onClick={() => void tokens.refetch()}>Try again</Button>} title="API tokens didn’t load">
          Check your connection and try again.
        </ErrorState>
      ) : (
        <>
          <Section id="new-token-heading" title="Create a token">
            {tokens.data.projects.length ? (
              <CreateTokenForm
                onCreated={(result) => {
                  setCreated(result)
                  void queryClient.invalidateQueries({ queryKey: accountKeys.apiTokens })
                }}
                projects={tokens.data.projects}
              />
            ) : (
              <p className="aw-quiet">You need to be a member of a Project to create a token for it.</p>
            )}
          </Section>
          <Section id="tokens-heading" title="Your tokens">
            {tokens.data.tokens.length ? (
              <RowList label="API tokens">
                {tokens.data.tokens.map((token) => (
                  <Row
                    actions={
                      token.status === 'active' ? (
                        <Button
                          disabled={revoke.isPending}
                          icon="x"
                          onClick={() => setRevoking(token)}
                          size="sm"
                          variant="ghost"
                        >
                          Revoke
                        </Button>
                      ) : null
                    }
                    className={token.status === 'active' ? undefined : 'is-disabled'}
                    key={token.id}
                    leading={
                      <span className="aw-row__tile">
                        <Icon name="key" size={16} />
                      </span>
                    }
                    meta={
                      <span className="aw-row__metaline aw-wrap">
                        <code className="aw-inline-code">{token.tokenPrefix}…</code>
                        <span>{token.projectName}</span>
                        <span>{token.scopes.includes('write') ? 'Read and write' : 'Read only'}</span>
                        <span>Created {formatShortDate(token.createdAt)}</span>
                        <span>{token.lastUsedAt ? `Used ${formatRelative(token.lastUsedAt)}` : 'Never used'}</span>
                        {token.status === 'active' && <span>Expires {formatShortDate(token.expiresAt)}</span>}
                      </span>
                    }
                    title={
                      <span className="aw-row__titleline">
                        {token.name}
                        <StatusChip tone={STATUS[token.status].tone}>{STATUS[token.status].label}</StatusChip>
                      </span>
                    }
                  />
                ))}
              </RowList>
            ) : (
              <EmptyState icon="key" title="No tokens yet">
                Create one above for a script or tool that should work as you.
              </EmptyState>
            )}
          </Section>
        </>
      )}
      <Dialog
        description={
          revoking
            ? `Anything using ${revoking.name} (${revoking.tokenPrefix}…) loses access to ${revoking.projectName} at once. This can’t be undone.`
            : undefined
        }
        footer={
          <>
            <Button onClick={() => setRevoking(null)}>Cancel</Button>
            <Button busy={revoke.isPending} data-autofocus onClick={() => revoking && revoke.mutate(revoking)} variant="danger">
              Revoke token
            </Button>
          </>
        }
        onClose={() => setRevoking(null)}
        open={revoking !== null}
        title="Revoke this token?"
      />
    </>
  )
}

function CreateTokenForm({
  onCreated,
  projects,
}: {
  onCreated: (result: { token: string; credential: ApiToken }) => void
  projects: ApiTokenProject[]
}) {
  const [projectId, setProjectId] = useState(projects[0]?.id ?? '')
  const [name, setName] = useState('')
  const [access, setAccess] = useState<'read' | 'write'>('read')
  const [days, setDays] = useState(30)
  const [error, setError] = useState<string | null>(null)
  const project = projects.find((candidate) => candidate.id === projectId) ?? projects[0]
  const canWrite = project?.canWrite ?? false
  const create = useMutation({
    mutationFn: () =>
      createApiToken({
        projectId: project.id,
        name: name.trim(),
        scopes: access === 'write' && canWrite ? ['read', 'write'] : ['read'],
        expiresInDays: days,
      }),
    onSuccess: (result) => {
      setName('')
      setError(null)
      onCreated(result)
    },
    onError: (reason) => setError(describeError(reason).message),
  })
  const trimmed = name.trim()
  const nameError = trimmed && (!NAME_PATTERN.test(trimmed) || trimmed.length > 80)
    ? 'Start with a letter or digit; use letters, digits, spaces and . _ - (up to 80).'
    : null
  const submit = (event: FormEvent<HTMLFormElement>) => {
    event.preventDefault()
    if (!trimmed || nameError || !project) return
    create.mutate()
  }

  return (
    <form className="aw-card aw-formcard aw-form" onSubmit={submit}>
      <div className="aw-form__grid">
        <TextField
          error={nameError}
          hint="So you can tell it apart later, such as “deploy script”."
          id="token-name"
          label="Name"
          maxLength={80}
          onChange={(event) => setName(event.target.value)}
          required
          value={name}
        />
        <SelectField id="token-project" label="Project" onChange={(event) => setProjectId(event.target.value)} value={project?.id ?? ''}>
          {projects.map((candidate) => (
            <option key={candidate.id} value={candidate.id}>
              {candidate.name}
            </option>
          ))}
        </SelectField>
      </div>
      <div className="aw-form__grid">
        <div className="aw-field">
          <span className="aw-field__label" id="token-access-label">
            Access
          </span>
          <SegmentedControl
            label="Access"
            onChange={setAccess}
            options={[
              { value: 'read', label: 'Read' },
              ...(canWrite ? [{ value: 'write' as const, label: 'Read and write' }] : []),
            ]}
            value={canWrite ? access : 'read'}
          />
          {!canWrite && <p className="aw-field__note">You can only read in this Project, so its tokens are read only.</p>}
        </div>
        <div className="aw-field">
          <span className="aw-field__label">Expires after</span>
          <div className="aw-token-expiry">
            <SegmentedControl
              label="Expires after"
              onChange={(value) => setDays(Number(value))}
              options={EXPIRY_PRESETS.map((preset) => ({ value: String(preset), label: preset === 365 ? '1 year' : `${preset} days` }))}
              value={String(days)}
            />
            <TextField
              aria-label="Days until it expires"
              inputMode="numeric"
              max={365}
              min={1}
              onChange={(event) => setDays(Math.max(1, Math.min(365, Math.round(Number(event.target.value) || 1))))}
              type="number"
              value={days}
            />
          </div>
        </div>
      </div>
      {error && <Banner tone="danger">{error}</Banner>}
      <div className="aw-form__actions">
        <Button busy={create.isPending} disabled={!trimmed || Boolean(nameError)} icon="plus" type="submit" variant="primary">
          Create token
        </Button>
      </div>
    </form>
  )
}
