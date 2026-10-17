import { useRef, useState, type ChangeEvent } from 'react'
import { useMutation, useQuery, useQueryClient } from '@tanstack/react-query'
import { Link } from 'react-router-dom'

import { describeError, toastError } from '../../app/mutations'
import { queryKeys } from '../../api/queryKeys'
import { listRepositories } from '../../repositories/api'
import {
  Banner,
  Button,
  Checkbox,
  ErrorState,
  ListSkeleton,
  PageHeader,
  Row,
  RowList,
  Section,
  SegmentedControl,
  SelectField,
  StatusChip,
  TextArea,
  formatRelative,
  showToast,
} from '../../ui'
import {
  commitImport,
  createImport,
  exportHref,
  getImport,
  listImports,
  updateImport,
  viewKeys,
  type ImportOptions,
  type ImportSource,
  type TaskImport,
} from '../../views/api'
import type { SettingsPageProps } from '../types'
import '../../views/views.css'

const FIELD_LABELS: Record<string, string> = {
  externalId: 'ID',
  title: 'Title',
  description: 'Description',
  status: 'Status',
  priority: 'Priority',
  labels: 'Labels',
  assignee: 'Assignee',
  type: 'Type',
  milestone: 'Milestone',
  startDate: 'Start date',
  dueDate: 'Due date',
  estimate: 'Estimate',
  parent: 'Parent',
}

const STATUS_LABELS: Record<TaskImport['status'], string> = {
  fetching: 'Reading from GitHub',
  draft: 'Ready to review',
  queued: 'Waiting to start',
  running: 'Importing',
  completed: 'Done',
  failed: 'Failed',
  canceled: 'Stopped',
}

const active = (taskImport: TaskImport) => ['fetching', 'queued', 'running'].includes(taskImport.status)

/** Project › Import and export: bring tasks in from a plan, a CSV or GitHub issues, and take them out again. */
export default function ImportExportPage({ project }: SettingsPageProps) {
  const projectId = project?.id ?? ''
  const imports = useQuery({ queryKey: viewKeys.imports(projectId), queryFn: () => listImports(projectId), enabled: Boolean(projectId) })
  const [openId, setOpenId] = useState<string | null>(null)
  const [includeArchived, setIncludeArchived] = useState(false)
  if (!project) return null
  const canImport = Boolean(imports.data?.canImport)
  const open = openId ?? imports.data?.imports.find((entry) => entry.status === 'draft' || active(entry))?.id ?? null

  return (
    <>
      <PageHeader icon="arrow-down" title="Import and export">
        Bring tasks into {project.name} from a Markdown plan, a CSV from Linear or Jira, or GitHub issues. Nothing is made until
        you’ve seen the preview.
      </PageHeader>
      {imports.isPending ? <ListSkeleton label="Loading imports" rows={2} /> : null}
      {imports.isError ? (
        <ErrorState action={<Button onClick={() => void imports.refetch()}>Try again</Button>} title="Imports didn’t load">
          Check your connection and try again.
        </ErrorState>
      ) : null}
      {imports.data && canImport && !open ? <NewImport onStarted={setOpenId} projectId={projectId} /> : null}
      {open ? <ImportReview importId={open} onDone={() => setOpenId('')} projectId={projectId} /> : null}
      {imports.data && !canImport ? (
        <Banner className="aw-page__banner" icon="info">
          Only members who can create tasks can import. You can still export.
        </Banner>
      ) : null}
      {imports.data?.imports.length ? (
        <Section id="past-imports-heading" title="Recent imports">
          <RowList label="Recent imports">
            {imports.data.imports.map((entry) => (
              <Row
                actions={
                  entry.status === 'draft' || active(entry) ? (
                    <Button onClick={() => setOpenId(entry.id)} size="sm" variant="ghost">
                      Open
                    </Button>
                  ) : entry.status === 'completed' ? (
                    <Link className="aw-btn aw-btn--ghost aw-btn--sm" to={entry.href}>
                      <span className="aw-btn__label">See tasks</span>
                    </Link>
                  ) : null
                }
                key={entry.id}
                meta={
                  <span className="aw-row__metaline aw-wrap">
                    <span>{STATUS_LABELS[entry.status]}</span>
                    {entry.status === 'completed' ? <span>{entry.createdCount.toLocaleString()} created</span> : null}
                    {entry.skippedCount ? <span>{entry.skippedCount.toLocaleString()} skipped</span> : null}
                    {entry.error ? <span>{entry.error}</span> : null}
                    <span>
                      {formatRelative(entry.createdAt)}
                      {entry.createdByName ? ` by ${entry.createdByName}` : ''}
                    </span>
                  </span>
                }
                title={entry.repositoryName ? `${entry.sourceName} from ${entry.repositoryName}` : entry.sourceName}
              />
            ))}
          </RowList>
        </Section>
      ) : null}
      <Section description="Every task with its fields, labels, criteria and links. JSON keeps everything; CSV opens in a spreadsheet." id="export-heading" title="Export">
        <div className="aw-form__row">
          <a className="aw-btn aw-btn--secondary aw-btn--sm" download href={exportHref(projectId, 'json', includeArchived)}>
            <span className="aw-btn__label">Download JSON</span>
          </a>
          <a className="aw-btn aw-btn--secondary aw-btn--sm" download href={exportHref(projectId, 'csv', includeArchived)}>
            <span className="aw-btn__label">Download CSV</span>
          </a>
          <Checkbox checked={includeArchived} label="Include archived tasks" onChange={setIncludeArchived} />
        </div>
      </Section>
    </>
  )
}

function NewImport({ onStarted, projectId }: { onStarted: (id: string) => void; projectId: string }) {
  const queryClient = useQueryClient()
  const idempotencyKey = useRef(crypto.randomUUID())
  const [source, setSource] = useState<ImportSource>('markdown')
  const [text, setText] = useState('')
  const [fileName, setFileName] = useState<string | undefined>()
  const [headings, setHeadings] = useState(true)
  const [repositoryId, setRepositoryId] = useState('')
  const [githubState, setGithubState] = useState<'open' | 'all'>('open')
  const [error, setError] = useState<string | null>(null)
  const repositories = useQuery({
    queryKey: queryKeys.repositories(projectId),
    queryFn: () => listRepositories(projectId),
    enabled: source === 'github',
  })
  const usable = (repositories.data?.repositories ?? []).filter((repository) => repository.enabled && !repository.archivedAt)
  const chosenRepository = repositoryId || usable[0]?.id || ''

  const start = useMutation({
    mutationFn: () =>
      createImport(
        projectId,
        source === 'github'
          ? { source, repositoryId: chosenRepository, options: { githubState } }
          : { source, text, fileName, options: { headingsAsMilestones: headings } },
        idempotencyKey.current,
      ),
    onSuccess: (created) => {
      idempotencyKey.current = crypto.randomUUID()
      void queryClient.invalidateQueries({ queryKey: viewKeys.imports(projectId) })
      onStarted(created.id)
    },
    onError: (reason) => {
      idempotencyKey.current = crypto.randomUUID()
      setError(describeError(reason).message)
    },
  })

  const readFile = (event: ChangeEvent<HTMLInputElement>) => {
    const file = event.target.files?.[0]
    if (!file) return
    if (file.size > 4 * 1024 * 1024) {
      setError('That file is over 4 MB. Split it and import the parts.')
      return
    }
    setError(null)
    setFileName(file.name)
    void file.text().then(setText)
  }

  const ready = source === 'github' ? Boolean(chosenRepository) : text.trim().length > 0
  return (
    <Section id="new-import-heading" title="New import">
      <div className="aw-form">
        <SegmentedControl
          label="Import from"
          onChange={(value) => {
            setSource(value)
            setText('')
            setFileName(undefined)
            setError(null)
          }}
          options={[
            { value: 'markdown', label: 'Markdown plan' },
            { value: 'csv', label: 'CSV' },
            { value: 'github', label: 'GitHub issues' },
          ]}
          value={source}
        />
        {source === 'markdown' ? (
          <>
            <TextArea
              error={error ?? undefined}
              hint="Each list item becomes a task; indented items become subtasks. “- [x]” items arrive done."
              label="Plan"
              onChange={(event) => setText(event.target.value)}
              placeholder={'## Beta\n- [ ] Ship the importer\n  - [ ] Parse CSV\n- [ ] Write the docs'}
              rows={8}
              value={text}
            />
            <Checkbox checked={headings} label="Headings become milestones" onChange={setHeadings} />
            <label className="aw-field__label">
              Or choose a file <input accept=".md,.markdown,.txt,text/markdown,text/plain" onChange={readFile} type="file" />
            </label>
          </>
        ) : null}
        {source === 'csv' ? (
          <>
            <label className="aw-field__label">
              CSV file <input accept=".csv,text/csv" onChange={readFile} type="file" />
            </label>
            <p className="aw-pagesection__desc">
              {fileName ? `${fileName} is ready.` : 'Exports from Linear and Jira map on their own; you can change the mapping in the preview.'}
            </p>
            {error ? (
              <Banner className="aw-page__banner" tone="danger">
                {error}
              </Banner>
            ) : null}
          </>
        ) : null}
        {source === 'github' ? (
          <>
            {repositories.isPending ? <ListSkeleton label="Loading repositories" rows={1} /> : null}
            {repositories.data && !usable.length ? (
              <Banner icon="info">Connect a repository to this Project first, under Repositories.</Banner>
            ) : null}
            {usable.length ? (
              <SelectField label="Repository" onChange={(event) => setRepositoryId(event.target.value)} value={chosenRepository}>
                {usable.map((repository) => (
                  <option key={repository.id} value={repository.id}>
                    {repository.owner}/{repository.name}
                  </option>
                ))}
              </SelectField>
            ) : null}
            <SelectField label="Issues" onChange={(event) => setGithubState(event.target.value as 'open' | 'all')} value={githubState}>
              <option value="open">Open issues</option>
              <option value="all">Open and closed issues</option>
            </SelectField>
            {error ? (
              <Banner className="aw-page__banner" tone="danger">
                {error}
              </Banner>
            ) : null}
          </>
        ) : null}
        <div className="aw-form__actions">
          <Button busy={start.isPending} disabled={!ready} onClick={() => start.mutate()} variant="primary">
            Preview import
          </Button>
        </div>
      </div>
    </Section>
  )
}

function ImportReview({ importId, onDone, projectId }: { importId: string; onDone: () => void; projectId: string }) {
  const queryClient = useQueryClient()
  const taskImport = useQuery({
    queryKey: viewKeys.taskImport(projectId, importId),
    queryFn: () => getImport(projectId, importId),
    refetchInterval: (query) => (query.state.data && active(query.state.data) ? 1500 : false),
  })
  const [finishedId, setFinishedId] = useState<string | null>(null)
  const data = taskImport.data
  if (data && !active(data) && data.status !== 'draft' && finishedId !== data.id) {
    setFinishedId(data.id)
    void queryClient.invalidateQueries({ queryKey: viewKeys.imports(projectId) })
    void queryClient.invalidateQueries({ queryKey: queryKeys.workItemsRoot(projectId) })
  }
  const refresh = (next: TaskImport) => {
    queryClient.setQueryData(viewKeys.taskImport(projectId, importId), next)
    void queryClient.invalidateQueries({ queryKey: viewKeys.imports(projectId) })
  }
  const change = useMutation({
    mutationFn: (changes: { mapping?: Record<string, string>; options?: ImportOptions; cancel?: boolean }) => {
      if (!data) throw new Error('The import isn’t loaded yet.')
      return updateImport(projectId, data, changes, crypto.randomUUID())
    },
    onSuccess: (next, changes) => {
      refresh(next)
      if (changes.cancel && next.status === 'canceled') onDone()
    },
    onError: (error) => {
      if (toastError('That didn’t save', error)) void taskImport.refetch()
    },
  })
  const commit = useMutation({
    mutationFn: () => {
      if (!data) throw new Error('The import isn’t loaded yet.')
      return commitImport(projectId, data, crypto.randomUUID())
    },
    onSuccess: (next) => {
      refresh(next)
      showToast({ title: `Importing ${next.preview?.willCreate ?? next.rowCount} tasks` })
    },
    onError: (error) => {
      if (toastError('The import didn’t start', error)) void taskImport.refetch()
    },
  })

  if (taskImport.isPending) return <ListSkeleton label="Loading the preview" rows={4} />
  if (taskImport.isError || !data) {
    return (
      <ErrorState action={<Button onClick={() => void taskImport.refetch()}>Try again</Button>} title="The preview didn’t load">
        Check your connection and try again.
      </ErrorState>
    )
  }
  const preview = data.preview
  const excluded = new Set(data.options.excludedRows ?? [])
  const setOption = (next: Partial<ImportOptions>) => change.mutate({ options: { ...data.options, ...next } })

  return (
    <Section
      actions={
        data.status === 'draft' || active(data) ? (
          <Button busy={change.isPending && change.variables?.cancel} onClick={() => change.mutate({ cancel: true })} size="sm" variant="ghost">
            {data.status === 'draft' ? 'Discard' : 'Stop'}
          </Button>
        ) : (
          <Button onClick={onDone} size="sm" variant="ghost">
            New import
          </Button>
        )
      }
      id="import-review-heading"
      title={data.repositoryName ? `${data.sourceName} from ${data.repositoryName}` : data.sourceName}
    >
      {data.status === 'fetching' ? (
        <p aria-live="polite" className="aw-pagesection__desc" role="status">
          Reading issues from {data.repositoryName ?? 'GitHub'}…
        </p>
      ) : null}
      {data.status === 'queued' || data.status === 'running' ? (
        <div aria-live="polite" role="status">
          <p className="aw-pagesection__desc">
            Imported {data.processedCount.toLocaleString()} of {data.rowCount.toLocaleString()}.
          </p>
          <progress max={Math.max(1, data.rowCount)} style={{ width: '100%' }} value={data.processedCount} />
        </div>
      ) : null}
      {data.status === 'completed' ? (
        <Banner className="aw-page__banner" icon="circle-check" tone="success">
          {data.createdCount.toLocaleString()} {data.createdCount === 1 ? 'task' : 'tasks'} created
          {data.skippedCount ? `, ${data.skippedCount.toLocaleString()} skipped` : ''}.{' '}
          <Link to={data.href}>See them</Link>
        </Banner>
      ) : null}
      {data.status === 'failed' || data.status === 'canceled' ? (
        <Banner className="aw-page__banner" tone={data.status === 'failed' ? 'danger' : 'info'}>
          {data.error ?? (data.status === 'failed' ? 'The import failed.' : 'The import was stopped.')}
          {data.createdCount ? ` ${data.createdCount.toLocaleString()} tasks were made before it ended; they stay.` : ''}
        </Banner>
      ) : null}
      {data.status === 'draft' && preview ? (
        <>
          <p className="aw-pagesection__desc">
            {preview.willCreate.toLocaleString()} of {preview.total.toLocaleString()} will be created.
            {preview.alreadyImported ? ` ${preview.alreadyImported.toLocaleString()} were imported before and stay as they are.` : ''}
            {preview.withoutTitle ? ` ${preview.withoutTitle.toLocaleString()} have no title.` : ''}
            {preview.leftOut ? ` ${preview.leftOut.toLocaleString()} left out.` : ''}
          </p>
          {data.columns ? (
            <fieldset style={{ border: 0, margin: 0, padding: 0 }}>
              <legend className="aw-field__label">Columns</legend>
              <div className="aw-form__row">
                {data.fields.map((field) => (
                  <SelectField
                    key={field}
                    label={FIELD_LABELS[field] ?? field}
                    onChange={(event) => {
                      const mapping = { ...data.mapping }
                      if (event.target.value) mapping[field] = event.target.value
                      else delete mapping[field]
                      change.mutate({ mapping })
                    }}
                    value={data.mapping[field] ?? ''}
                  >
                    <option value="">Not imported</option>
                    {data.columns?.map((column) => (
                      <option key={column} value={column}>
                        {column}
                      </option>
                    ))}
                  </SelectField>
                ))}
              </div>
            </fieldset>
          ) : null}
          {preview.newLabels.length ? (
            <Checkbox
              checked={data.options.createLabels}
              description={preview.newLabels.slice(0, 20).join(', ') + (preview.newLabels.length > 20 ? '…' : '')}
              label={`Create ${preview.newLabels.length} new ${preview.newLabels.length === 1 ? 'label' : 'labels'}`}
              onChange={(checked) => setOption({ createLabels: checked })}
            />
          ) : null}
          {preview.newMilestones.length ? (
            <Checkbox
              checked={data.options.createMilestones}
              description={preview.newMilestones.slice(0, 20).join(', ')}
              label={`Create ${preview.newMilestones.length} new ${preview.newMilestones.length === 1 ? 'milestone' : 'milestones'}`}
              onChange={(checked) => setOption({ createMilestones: checked })}
            />
          ) : null}
          {Object.entries(preview.unmatched).map(([field, values]) => (
            <p className="aw-pagesection__desc" key={field}>
              No match for {FIELD_LABELS[field]?.toLowerCase() ?? field}{' '}
              {Object.entries(values)
                .slice(0, 8)
                .map(([value, count]) => `“${value}” (${count})`)
                .join(', ')}
              ; those use the default.
            </p>
          ))}
          <ul aria-label="Rows to import" className="aw-mywork__list">
            {preview.rows.map((row) => (
              <li className="aw-mywork__row" key={row.index}>
                <Checkbox
                  checked={!row.skip}
                  disabled={Boolean(row.skip && row.skip !== 'Left out')}
                  label={<span className={row.parent !== null ? 'aw-import__child' : undefined}>{row.title || '(no title)'}</span>}
                  onChange={(checked) =>
                    setOption({ excludedRows: checked ? [...excluded].filter((index) => index !== row.index) : [...excluded, row.index] })
                  }
                />
                <span className="aw-mywork__meta">
                  {row.skip && row.skip !== 'Left out' ? <StatusChip>{row.skip}</StatusChip> : null}
                  {row.status ? <span>{row.status}</span> : null}
                  {row.labels.length ? <span>{row.labels.join(', ')}</span> : null}
                  {row.milestone ? <span>{row.milestone}</span> : null}
                  {row.dueDate ? <span>Due {row.dueDate}</span> : null}
                </span>
              </li>
            ))}
          </ul>
          {preview.total > preview.rowsShown ? (
            <p className="aw-mywork__none">
              Showing the first {preview.rowsShown.toLocaleString()} of {preview.total.toLocaleString()}.
            </p>
          ) : null}
          <div className="aw-form__actions">
            <Button busy={commit.isPending} disabled={!preview.willCreate || change.isPending} onClick={() => commit.mutate()} variant="primary">
              Import {preview.willCreate.toLocaleString()} {preview.willCreate === 1 ? 'task' : 'tasks'}
            </Button>
          </div>
        </>
      ) : null}
    </Section>
  )
}
