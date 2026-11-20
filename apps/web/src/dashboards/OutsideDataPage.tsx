import { useState } from 'react'
import { useQuery } from '@tanstack/react-query'
import { Link, useParams, useSearchParams } from 'react-router-dom'

import type { OpOutput } from '../api/ops'
import { opQuery, useOp } from '../api/ops'
import { ProjectPageError, ProjectPageLoading } from '../projects/ProjectPageStates'
import { useProject } from '../projects/hooks'
import { useShellPage } from '../shell/shellStore'
import {
  Banner,
  Button,
  ButtonLink,
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
  SelectField,
  StatusChip,
  Switch,
  TextArea,
  TextField,
  countLabel,
  describeError,
  formatRelative,
  showToast,
} from '../ui'
import { AnswerTable } from './AnswerTable'
import { dashboardsHref } from './dashboardModel'
import {
  answerWords,
  dataQueryHref,
  outsideDataHref,
  parameterText,
  parameterValue,
  usedParameters,
  type ParameterDraft,
  type ParameterType,
} from './outsideDataModel'

type QueryEntry = OpOutput<'data_query_get'>
type ConnectionEntry = OpOutput<'data_connection_list'>['connections'][number]

/**
 * Outside data (T1.34, T1.35): a project's connections to databases kept
 * elsewhere, and saved read-only queries whose answers are stored for its
 * dashboards. Roles that connect data write, run and review queries; a
 * reviewed query can be shared, and then everyone sees its answer.
 */
export function OutsideDataPage() {
  const { projectId = '' } = useParams()
  const [search, setSearch] = useSearchParams()
  const project = useProject(projectId)
  const list = useQuery({ ...opQuery('data_query_list', { spaceId: projectId }), enabled: Boolean(projectId) })
  const canConnect = list.data?.canConnect === true
  const connections = useQuery({ ...opQuery('data_connection_list', { spaceId: projectId }), enabled: canConnect })
  const [connecting, setConnecting] = useState<ConnectionEntry | 'new' | null>(null)
  const selected = search.get('query')

  useShellPage({ project: project.data })
  if (project.isPending) return <ProjectPageLoading label="Loading outside data" />
  if (project.isError) return <ProjectPageError error={project.error} onRetry={() => void project.refetch()} />

  const open = !project.data.archivedAt
  const queries = list.data?.queries ?? []
  const choose = (id: string | null) => setSearch(id ? { query: id } : {})
  const actions =
    open && canConnect ? (
      <>
        <Button icon="link" onClick={() => setConnecting('new')}>
          Connect a database
        </Button>
        <Button disabled={!connections.data?.connections.length} icon="plus" onClick={() => choose('new')} variant="primary">
          New query
        </Button>
      </>
    ) : null

  return (
    <div className="aw-page aw-outside">
      <PageHeader actions={actions} icon="table" title="Outside data">
        <p>
          Saved queries over databases kept elsewhere. Their answers are stored, so <Link to={dashboardsHref(projectId)}>dashboards</Link> show them without reaching the database.
        </p>
      </PageHeader>
      {list.isPending ? <ListSkeleton label="Loading queries" rows={3} /> : null}
      {list.isError ? <ErrorState error={list.error} onRetry={() => void list.refetch()} title="Queries didn’t load" /> : null}
      {selected ? (
        <QueryPanel
          canConnect={canConnect}
          canShare={list.data?.canShare === true}
          connections={connections.data?.connections ?? []}
          key={selected}
          onClose={() => choose(null)}
          onSaved={(id) => choose(id)}
          open={open}
          projectId={projectId}
          queryId={selected === 'new' ? null : selected}
        />
      ) : null}
      {list.isSuccess && queries.length === 0 && !selected ? (
        <EmptyState action={actions} icon="table" title={canConnect ? 'No saved queries yet' : 'Nothing shared yet'}>
          {canConnect
            ? 'Connect a database, then save a query over it. Queries only read: they run in a read-only transaction, with a time limit and a cap on the rows kept.'
            : 'When someone who connects data shares a reviewed query, its answer shows here and on dashboards.'}
        </EmptyState>
      ) : null}
      {queries.length ? (
        <Section title="Queries">
          <RowList label="Saved queries">
            {queries.map((query) => (
              <Row
                key={query.id}
                leading={<Icon name="table" size={18} />}
                meta={[query.answer?.ranAt ? `ran ${formatRelative(query.answer.ranAt)}` : 'not run yet', query.description || null].filter(Boolean).join(' · ')}
                title={<Link to={dataQueryHref(projectId, query.id)}>{query.name}</Link>}
                actions={<QueryChips query={query} />}
              />
            ))}
          </RowList>
        </Section>
      ) : null}
      {canConnect ? (
        <Section title="Connections">
          {connections.isError ? <ErrorState error={connections.error} onRetry={() => void connections.refetch()} title="Connections didn’t load" /> : null}
          {connections.data && connections.data.connections.length === 0 ? <p className="aw-outside__quiet">No databases connected yet.</p> : null}
          {connections.data?.connections.length ? (
            <RowList label="Connections">
              {connections.data.connections.map((connection) => (
                <Row
                  key={connection.id}
                  leading={<Icon name="link" size={18} />}
                  meta={`${connection.address} · ${countLabel(connection.queries, 'query', 'queries')}`}
                  title={connection.name}
                  actions={
                    open ? (
                      <Button onClick={() => setConnecting(connection)} size="sm" variant="ghost">
                        Edit
                      </Button>
                    ) : null
                  }
                />
              ))}
            </RowList>
          ) : null}
        </Section>
      ) : null}
      {connecting ? <ConnectionDialog connection={connecting === 'new' ? null : connecting} onClose={() => setConnecting(null)} projectId={projectId} /> : null}
    </div>
  )
}

function QueryChips({ query }: { query: Pick<QueryEntry, 'review' | 'shared'> }) {
  return (
    <span className="aw-outside__chips">
      {query.review === 'reviewed' ? (
        <StatusChip icon="check" tone="success">
          Reviewed
        </StatusChip>
      ) : (
        <StatusChip icon="pencil" tone="warning">
          Draft
        </StatusChip>
      )}
      {query.shared ? (
        <StatusChip icon="users" tone="muted">
          Shared
        </StatusChip>
      ) : null}
    </span>
  )
}

/** Adds a connection, or renames one or gives it a new address. */
function ConnectionDialog({ connection, onClose, projectId }: { connection: ConnectionEntry | null; onClose: () => void; projectId: string }) {
  const [name, setName] = useState(connection?.name ?? '')
  const [address, setAddress] = useState('')
  const [problem, setProblem] = useState<string | null>(null)
  const [removing, setRemoving] = useState(false)
  const invalidates = ['data_connection_list', 'data_query_list', 'data_query_get'] as const
  const create = useOp('data_connection_create', { invalidates: [...invalidates] })
  const update = useOp('data_connection_update', { invalidates: [...invalidates] })
  const remove = useOp('data_connection_delete', { invalidates: [...invalidates] })
  const submit = async () => {
    setProblem(null)
    if (!name.trim()) return setProblem('Give the connection a name.')
    if (!connection && !address.trim()) return setProblem('Give the database’s address.')
    try {
      if (connection) {
        await update.mutateAsync({ connection: connection.id, expectedVersion: connection.version, name: name.trim(), address: address.trim() || null })
      } else {
        await create.mutateAsync({ spaceId: projectId, name: name.trim(), address: address.trim() })
      }
      onClose()
    } catch (error) {
      setProblem(describeError(error).message)
    }
  }
  return (
    <>
      <Dialog
        dirty
        footer={
          <>
            {connection ? (
              <Button onClick={() => setRemoving(true)} variant="danger">
                Remove
              </Button>
            ) : null}
            <Button onClick={onClose} variant="ghost">
              Cancel
            </Button>
            <Button busy={create.isPending || update.isPending} type="submit" variant="primary">
              {connection ? 'Save' : 'Connect'}
            </Button>
          </>
        }
        onClose={onClose}
        onSubmit={(event) => {
          event.preventDefault()
          void submit()
        }}
        open
        title={connection ? `Edit ${connection.name}` : 'Connect a database'}
      >
        {problem ? <Banner tone="danger">{problem}</Banner> : null}
        <TextField autoFocus hint="Such as Warehouse or Billing replica." label="Name" maxLength={80} onChange={(event) => setName(event.target.value)} required value={name} />
        <TextField
          autoComplete="off"
          hint={
            connection
              ? `Now ${connection.address}. Leave empty to keep it; a new address makes its queries drafts until they’re reviewed again.`
              : 'postgresql://user:password@host:5432/database. It’s kept as a secret and never shown again. Connect as a user that can only read.'
          }
          label={connection ? 'New address' : 'Address'}
          maxLength={1000}
          mono
          onChange={(event) => setAddress(event.target.value)}
          required={!connection}
          spellCheck={false}
          type="password"
          value={address}
        />
      </Dialog>
      {removing && connection ? (
        <ConfirmDialog
          busy={remove.isPending}
          confirmLabel="Remove connection"
          onClose={() => setRemoving(false)}
          onConfirm={() =>
            void remove
              .mutateAsync({ connection: connection.id })
              .then(onClose)
              .catch((error: unknown) => {
                setRemoving(false)
                setProblem(describeError(error).message)
              })
          }
          open
          title={`Remove ${connection.name}?`}
          tone="danger"
        >
          Its address is forgotten. Queries that run on it must be removed or moved first.
        </ConfirmDialog>
      ) : null}
    </>
  )
}

const PARAMETER_TYPES: { value: ParameterType; label: string }[] = [
  { value: 'text', label: 'Text' },
  { value: 'number', label: 'Number' },
  { value: 'date', label: 'Date' },
  { value: 'boolean', label: 'Yes or no' },
]

/** One saved query: its editor for roles that connect data, its answer for everyone. */
function QueryPanel({
  canConnect,
  canShare,
  connections,
  onClose,
  onSaved,
  open,
  projectId,
  queryId,
}: {
  canConnect: boolean
  canShare: boolean
  connections: ConnectionEntry[]
  onClose: () => void
  onSaved: (id: string) => void
  open: boolean
  projectId: string
  queryId: string | null
}) {
  const query = useQuery({
    ...opQuery('data_query_get', { query: queryId ?? '' }),
    enabled: Boolean(queryId),
    refetchInterval: (state) => (state.state.data?.answer?.running ? 1500 : false),
  })
  if (queryId && query.isPending) return <ListSkeleton label="Loading the query" rows={4} />
  if (queryId && query.isError) return <ErrorState error={query.error} onRetry={() => void query.refetch()} title="The query didn’t load" />
  const current = query.data ?? null
  return (
    <Section
      actions={
        <Button icon="x" onClick={onClose} size="sm" variant="ghost">
          Close
        </Button>
      }
      title={current ? current.name : 'New query'}
    >
      <div className="aw-outside__panel">
        {canConnect && open ? (
          <QueryEditor connections={connections} onSaved={onSaved} projectId={projectId} query={current} />
        ) : current?.description ? (
          <p>{current.description}</p>
        ) : null}
        {current ? <QueryReview canConnect={canConnect && open} canShare={canShare && open} query={current} onRemoved={onClose} /> : null}
        {current ? <QueryAnswer open={open} query={current} /> : null}
      </div>
    </Section>
  )
}

function QueryEditor({ connections, onSaved, projectId, query }: { connections: ConnectionEntry[]; onSaved: (id: string) => void; projectId: string; query: QueryEntry | null }) {
  const [name, setName] = useState(query?.name ?? '')
  const [description, setDescription] = useState(query?.description ?? '')
  const [connection, setConnection] = useState(query?.connection ?? connections[0]?.id ?? '')
  const [statement, setStatement] = useState(query?.statement ?? '')
  const [rowCap, setRowCap] = useState(String(query?.rowCap ?? 1000))
  const [parameters, setParameters] = useState<ParameterDraft[]>(
    (query?.parameters ?? []).map((parameter) => ({ name: parameter.name, type: parameter.type, value: parameterText(parameter.default) })),
  )
  const [problem, setProblem] = useState<string | null>(null)
  const invalidates = ['data_query_list', 'data_query_get', 'data_connection_list', 'dashboard_get'] as const
  const create = useOp('data_query_create', { invalidates: [...invalidates] })
  const update = useOp('data_query_update', { invalidates: [...invalidates] })
  const used = usedParameters(statement)
  const missing = used.filter((name) => !parameters.some((parameter) => parameter.name === name))
  const submit = async () => {
    setProblem(null)
    if (!name.trim()) return setProblem('Give the query a name.')
    if (!connection) return setProblem('Pick the connection it runs on.')
    if (!statement.trim()) return setProblem('Write the query: one SELECT.')
    const cap = Number(rowCap)
    const written = {
      name: name.trim(),
      description: description.trim(),
      connection,
      statement,
      rowCap: Number.isInteger(cap) ? cap : 1000,
      parameters: parameters.map((parameter) => ({ name: parameter.name.trim(), type: parameter.type, default: parameterValue(parameter.type, parameter.value) })),
    }
    try {
      if (query) {
        await update.mutateAsync({ query: query.id, expectedVersion: query.version, ...written })
      } else {
        const made = await create.mutateAsync({ spaceId: projectId, ...written })
        onSaved(made.id)
      }
      showToast({ tone: 'success', title: query && query.review === 'reviewed' && statement !== query.statement ? 'Saved as a draft: review it again' : 'Query saved' })
    } catch (error) {
      setProblem(describeError(error).message)
    }
  }
  return (
    <form
      className="aw-outside__editor"
      onSubmit={(event) => {
        event.preventDefault()
        void submit()
      }}
    >
      {problem ? <Banner tone="danger">{problem}</Banner> : null}
      <div className="aw-outside__row">
        <TextField label="Name" maxLength={80} onChange={(event) => setName(event.target.value)} required value={name} />
        <SelectField label="Connection" onChange={(event) => setConnection(event.target.value)} required value={connection}>
          <option value="">Pick a connection</option>
          {connections.map((entry) => (
            <option key={entry.id} value={entry.id}>
              {entry.name}
            </option>
          ))}
        </SelectField>
      </div>
      <TextField hint="Optional." label="Description" maxLength={2000} onChange={(event) => setDescription(event.target.value)} value={description} />
      <TextArea
        className="aw-outside__sql"
        hint="One SELECT. Write {{name}} for a parameter; its value is bound, never pasted in."
        label="Query"
        maxLength={20000}
        onChange={(event) => setStatement(event.target.value)}
        required
        rows={8}
        spellCheck={false}
        value={statement}
      />
      <fieldset className="aw-outside__parameters">
        <legend>Parameters</legend>
        {parameters.map((parameter, index) => (
          <div className="aw-outside__row" key={index}>
            <TextField
              label="Name"
              maxLength={40}
              onChange={(event) => setParameters(parameters.map((entry, at) => (at === index ? { ...entry, name: event.target.value } : entry)))}
              value={parameter.name}
            />
            <SelectField
              label="Type"
              onChange={(event) => setParameters(parameters.map((entry, at) => (at === index ? { ...entry, type: event.target.value as ParameterType } : entry)))}
              value={parameter.type}
            >
              {PARAMETER_TYPES.map((type) => (
                <option key={type.value} value={type.value}>
                  {type.label}
                </option>
              ))}
            </SelectField>
            <TextField
              hint={parameter.type === 'date' ? 'Such as 2026-11-19.' : parameter.type === 'boolean' ? 'true or false.' : undefined}
              label="Default"
              onChange={(event) => setParameters(parameters.map((entry, at) => (at === index ? { ...entry, value: event.target.value } : entry)))}
              value={parameter.value}
            />
            <Button aria-label={`Remove ${parameter.name || 'parameter'}`} icon="trash" onClick={() => setParameters(parameters.filter((_, at) => at !== index))} variant="ghost" />
          </div>
        ))}
        {missing.length ? (
          <p className="aw-outside__quiet">
            The query uses {missing.map((name) => `{{${name}}}`).join(', ')}.{' '}
            <Button onClick={() => setParameters([...parameters, ...missing.map((name) => ({ name, type: 'text' as const, value: '' }))])} size="sm" variant="ghost">
              Add {missing.length === 1 ? 'it' : 'them'}
            </Button>
          </p>
        ) : null}
        <Button icon="plus" onClick={() => setParameters([...parameters, { name: '', type: 'text', value: '' }])} size="sm" variant="ghost">
          Add a parameter
        </Button>
      </fieldset>
      <div className="aw-outside__row">
        <div className="aw-outside__narrow">
          <TextField hint="Rows kept of an answer, 1 to 5,000." inputMode="numeric" label="Row cap" onChange={(event) => setRowCap(event.target.value)} value={rowCap} />
        </div>
      </div>
      <div className="aw-outside__actions">
        <Button busy={create.isPending || update.isPending} type="submit" variant="primary">
          {query ? 'Save' : 'Save query'}
        </Button>
      </div>
    </form>
  )
}

function QueryReview({ canConnect, canShare, onRemoved, query }: { canConnect: boolean; canShare: boolean; onRemoved: () => void; query: QueryEntry }) {
  const invalidates = ['data_query_list', 'data_query_get', 'dashboard_get'] as const
  const review = useOp('data_query_review', { invalidates: [...invalidates] })
  const share = useOp('data_query_share', { invalidates: [...invalidates] })
  const remove = useOp('data_query_delete', { invalidates: [...invalidates, 'data_connection_list'] })
  const [removing, setRemoving] = useState(false)
  const act = async (work: () => Promise<unknown>, failed: string) => {
    try {
      await work()
    } catch (error) {
      showToast({ tone: 'danger', title: failed, description: describeError(error).message })
    }
  }
  return (
    <div className="aw-outside__review">
      <QueryChips query={query} />
      <span className="aw-outside__quiet">
        {query.review === 'reviewed'
          ? `Reviewed ${query.reviewedAt ? formatRelative(query.reviewedAt) : ''}. A change to what it runs makes it a draft again.`
          : 'A draft: only people who connect data run it, until someone reviews it.'}
      </span>
      {canConnect ? (
        <Button
          busy={review.isPending}
          onClick={() => void act(() => review.mutateAsync({ query: query.id, expectedVersion: query.version, state: query.review === 'reviewed' ? 'draft' : 'reviewed' }), 'The review didn’t save')}
          size="sm"
          variant={query.review === 'reviewed' ? 'ghost' : 'secondary'}
        >
          {query.review === 'reviewed' ? 'Back to draft' : 'Mark reviewed'}
        </Button>
      ) : null}
      {canShare ? (
        <Switch
          checked={query.shared}
          description={query.review === 'reviewed' || query.shared ? 'Everyone in the project sees its last reviewed answer, and may run it again.' : 'Review it first to share it.'}
          disabled={share.isPending || (!query.shared && query.review !== 'reviewed')}
          label="Share its answer"
          onChange={(shared) => void act(() => share.mutateAsync({ query: query.id, shared }), 'Sharing didn’t change')}
        />
      ) : null}
      {canConnect ? (
        <Button icon="trash" onClick={() => setRemoving(true)} size="sm" variant="ghost">
          Remove
        </Button>
      ) : null}
      {removing ? (
        <ConfirmDialog
          busy={remove.isPending}
          confirmLabel="Remove query"
          onClose={() => setRemoving(false)}
          onConfirm={() =>
            void act(() => remove.mutateAsync({ query: query.id }), 'The query wasn’t removed').then(() => {
              setRemoving(false)
              onRemoved()
            })
          }
          open
          title={`Remove ${query.name}?`}
          tone="danger"
        >
          Its stored answers go with it, and dashboard tiles that showed it say it’s gone.
        </ConfirmDialog>
      ) : null}
    </div>
  )
}

function QueryAnswer({ open, query }: { open: boolean; query: QueryEntry }) {
  const run = useOp('data_query_run', { invalidates: ['data_query_get', 'data_query_list'] })
  const answer = query.answer ?? null
  const running = run.isPending || answer?.running === true
  return (
    <div className="aw-outside__answer">
      <div className="aw-outside__answerhead">
        <h3>Answer</h3>
        {answer?.ranAt ? <span className="aw-outside__quiet">{answerWords(answer)}{answer.reviewed ? '' : ' · draft run'}</span> : null}
        {query.canRun && open ? (
          <Button
            busy={running}
            icon="play"
            onClick={() =>
              void run.mutateAsync({ query: query.id }).catch((error: unknown) => showToast({ tone: 'danger', title: 'The query didn’t start', description: describeError(error).message }))
            }
            size="sm"
          >
            {running ? 'Running' : answer?.ranAt ? 'Run again' : 'Run'}
          </Button>
        ) : null}
      </div>
      {answer?.error ? <Banner tone="danger">{answer.error}</Banner> : null}
      {answer && !answer.error && answer.ranAt ? <AnswerTable answer={answer} /> : null}
      {!answer?.ranAt && !running ? <p className="aw-outside__quiet">It hasn’t run yet.</p> : null}
    </div>
  )
}

export function OutsideDataLink({ projectId }: { projectId: string }) {
  return (
    <ButtonLink icon="table" to={outsideDataHref(projectId)}>
      Outside data
    </ButtonLink>
  )
}
