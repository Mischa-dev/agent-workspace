import { useState, type FormEvent } from 'react'
import { useMutation, useQuery, useQueryClient } from '@tanstack/react-query'
import { Link } from 'react-router-dom'

import { describeError } from '../../app/mutations'
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
  SegmentedControl,
  SelectField,
  StatusChip,
  TextField,
  formatFullDate,
  formatRelative,
  formatShortDate,
  showToast,
  type ChipTone,
} from '../../ui'
import { accountSettingsHref } from '../paths'
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

/** Account › API tokens: personal tokens for scripts and tools, one project each, acting as you. */
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
    onSettled: () => queryClient.invalidateQueries({ queryKey: accountKeys.apiTokens }),
  })

  return (
    <>
      <PageHeader icon="key" title="API tokens">
        Tokens let scripts and tools reach one project as you, through the MCP server or the REST API (see{' '}
        <Link to={accountSettingsHref('api-reference')}>API reference</Link>). Treat them like passwords.
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
        <ErrorState error={tokens.error} onRetry={() => void tokens.refetch()} title="API tokens didn’t load" />
      ) : (
        <>
          <Section id="new-token-heading" title="New token">
            {tokens.data.projects.length ? (
              <CreateTokenForm
                onCreated={(result) => {
                  setCreated(result)
                  void queryClient.invalidateQueries({ queryKey: accountKeys.apiTokens })
                }}
                projects={tokens.data.projects}
              />
            ) : (
              <p className="aw-quiet">Tokens reach one project. Join a project as a member to create one.</p>
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
                        <span title={formatFullDate(token.createdAt)}>Created {formatShortDate(token.createdAt)}</span>
                        {token.lastUsedAt ? (
                          <span title={formatFullDate(token.lastUsedAt)}>Used {formatRelative(token.lastUsedAt)}</span>
                        ) : (
                          <span>Never used</span>
                        )}
                        {token.status === 'active' && (
                          <span title={formatFullDate(token.expiresAt)}>Expires {formatShortDate(token.expiresAt)}</span>
                        )}
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
      <ConfirmDialog
        busy={revoke.isPending}
        confirmLabel="Revoke token"
        error={revoke.isError ? `The token wasn’t revoked. ${describeError(revoke.error).message}` : null}
        onClose={() => {
          setRevoking(null)
          revoke.reset()
        }}
        onConfirm={() => revoking && revoke.mutate(revoking)}
        open={revoking !== null}
        title={`Revoke ${revoking?.name ?? 'this token'}?`}
      >
        {revoking
          ? `Anything using it (${revoking.tokenPrefix}…) loses access to ${revoking.projectName} right away. This can’t be undone.`
          : null}
      </ConfirmDialog>
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
  // The field keeps what's typed, so it can be cleared and retyped; it's checked on submit.
  const [daysText, setDaysText] = useState('30')
  const days = Number(daysText)
  const daysValid = Number.isInteger(days) && days >= 1 && days <= 365
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
    onError: (reason) => setError(`The token wasn’t created. ${describeError(reason).message}`),
  })
  const trimmed = name.trim()
  const nameError = trimmed && (!NAME_PATTERN.test(trimmed) || trimmed.length > 80)
    ? 'Start with a letter or digit; use letters, digits, spaces and . _ - (up to 80).'
    : null
  const [daysChecked, setDaysChecked] = useState(false)
  const submit = (event: FormEvent<HTMLFormElement>) => {
    event.preventDefault()
    setDaysChecked(true)
    if (!trimmed || nameError || !project || !daysValid) return
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
          {!canWrite && <p className="aw-field__note">You can only read in this project, so its tokens are read only.</p>}
        </div>
        <div className="aw-field">
          <span className="aw-field__label">Expires after</span>
          <div className="aw-token-expiry">
            <SegmentedControl
              label="Expires after"
              onChange={setDaysText}
              options={EXPIRY_PRESETS.map((preset) => ({ value: String(preset), label: preset === 365 ? '1 year' : `${preset} days` }))}
              value={daysText}
            />
            <TextField
              aria-label="Days until it expires"
              error={daysChecked && !daysValid ? 'Enter a number of days from 1 to 365.' : null}
              inputMode="numeric"
              max={365}
              min={1}
              onChange={(event) => setDaysText(event.target.value)}
              type="number"
              value={daysText}
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
