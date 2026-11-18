import { useRef, useState, type FormEvent, type ReactNode } from 'react'
import { useQuery } from '@tanstack/react-query'
import { Link, useSearchParams } from 'react-router-dom'

import { requestJson } from '../api/client'
import { queryKeys } from '../api/queryKeys'
import { useProjects } from '../projects/hooks'
import { useShellPage } from '../shell/shellStore'
import {
  Banner,
  Button,
  EmptyState,
  Icon,
  ListSkeleton,
  PageHeader,
  Row,
  RowList,
  StatusChip,
  TextField,
  describeLoadError,
  formatFullDate,
  formatRelative,
  plainPreview,
  withEllipses,
} from '../ui'
import { useListKeys } from '../ui/listKeys'
import { describeFilter, queryFilters, withToken, withoutToken } from './searchQuery'
import '../styles/pages.css'

type SearchHit = {
  kind: 'message' | 'thread' | 'task' | 'page' | 'decision' | 'record' | 'file' | 'whiteboard' | 'sheet'
  id: string
  title: string
  /** A thread's or task's status, or "superseded" for a replaced decision. */
  status?: string | null
  snippet: string
  /** Null for a workspace's own board, which is in no project. */
  projectId: string | null
  workspaceId: string
  projectName: string
  conversationName: string | null
  author: string | null
  at: string
  path: string
}

const KIND_LABEL: Record<SearchHit['kind'], string> = {
  decision: 'Decision',
  page: 'Page',
  thread: 'Thread',
  task: 'Task',
  record: 'Record',
  message: 'Message',
  file: 'File',
  whiteboard: 'Board',
  sheet: 'Sheet',
}

/** The title and its status apart. Servers before `status` was its own field put it in the title's last brackets. */
function titleAndStatus(hit: SearchHit): { title: string; status: string | null } {
  if (hit.status !== undefined) return { title: hit.title, status: hit.status }
  if (hit.kind === 'thread' || hit.kind === 'task') {
    const match = /^(.*) \(([^()]+)\)$/s.exec(hit.title)
    if (match) return { title: match[1], status: match[2] }
  }
  if (hit.kind === 'decision' && hit.title.endsWith(' (superseded)')) {
    return { title: hit.title.slice(0, -' (superseded)'.length), status: 'superseded' }
  }
  return { title: hit.title, status: null }
}

/** Filters offered as one click under the box, when they aren't in the query already. */
const QUICK_FILTERS = [
  { token: 'from:me', label: 'From you' },
  { token: 'is:dm', label: 'In DMs' },
  { token: 'is:unread', label: 'Unread' },
  { token: 'is:saved', label: 'Saved' },
  { token: 'has:file', label: 'Has a file' },
  { token: 'has:link', label: 'Has a link' },
]

const capitalized = (text: string) => text.charAt(0).toUpperCase() + text.slice(1)

const START = '\u0001'
const STOP = '\u0002'

/** The snippet as plain text, with the words that matched (the server wraps them in `**`) marked. */
function Snippet({ text }: { text: string }) {
  const plain = withEllipses(plainPreview(text.replace(/\*\*([\s\S]*?)\*\*/g, `${START}$1${STOP}`), 320))
  const parts: ReactNode[] = []
  for (const [index, part] of plain.split(new RegExp(`(${START}[^${STOP}]*${STOP})`)).entries()) {
    if (!part) continue
    if (part.startsWith(START)) parts.push(<mark key={index}>{part.slice(1, -1)}</mark>)
    else parts.push(part.replaceAll(START, '').replaceAll(STOP, ''))
  }
  return <p className="aw-row__text aw-search__snippet">{parts}</p>
}

/** Search everything you can see: messages, threads, tasks, pages, decisions and files by name. */
export function SearchPage() {
  const [params, setParams] = useSearchParams()
  const query = params.get('q') ?? ''
  const projectId = params.get('projectId') ?? ''
  const projects = useProjects()
  const scope = projectId ? (projects.data?.projects.find((project) => project.id === projectId) ?? null) : null
  // The box follows the address, so Back shows what was searched then.
  const [draft, setDraft] = useState(query)
  const [shownQuery, setShownQuery] = useState(query)
  if (shownQuery !== query) {
    setShownQuery(query)
    setDraft(query)
  }
  const results = useQuery({
    queryKey: queryKeys.search(query, projectId),
    queryFn: () => {
      const search = new URLSearchParams({ q: query })
      if (projectId) search.set('projectId', projectId)
      return requestJson<{ hits: SearchHit[] }>(`/api/v1/search?${search}`)
    },
    enabled: query.trim().length > 0,
  })
  const submit = (event: FormEvent) => {
    event.preventDefault()
    const next = new URLSearchParams(params)
    next.set('q', draft.trim())
    setParams(next)
  }
  // Down and Up (or J and K) step through the results once you leave the box.
  const resultsRef = useRef<HTMLDivElement>(null)
  useListKeys(resultsRef, '.aw-row__link')
  useShellPage({ mainClassName: 'aw-main--page', project: scope })
  const filters = queryFilters(query)
  const searchFor = (next: string) => {
    const search = new URLSearchParams(params)
    search.set('q', next)
    setParams(search)
  }
  const searchEverything = () => {
    const next = new URLSearchParams(params)
    next.delete('projectId')
    setParams(next)
  }
  return (
    <>
      <div className="aw-page aw-page--reading">
        <PageHeader icon="search" title="Search" />
        <form className="aw-inline-form aw-search__form" onSubmit={submit} role="search">
          <TextField
            aria-label={scope ? `Search ${scope.name}` : 'Search'}
            autoFocus
            id="search-query"
            maxLength={200}
            onChange={(event) => setDraft(event.target.value)}
            placeholder="theming decision, AWD-12, “exact phrase”, from:@name…"
            value={draft}
          />
          <Button disabled={!draft.trim()} icon="search" type="submit" variant="primary">
            Search
          </Button>
        </form>
        <div className="aw-search__filters">
          {filters.length ? (
            <ul aria-label="Filters in this search" className="aw-search__chips">
              {filters.map((filter) => (
                <li className={filter.ok ? 'aw-search__chip' : 'aw-search__chip aw-search__chip--wrong'} key={filter.token}>
                  <span title={filter.ok ? filter.token : 'This filter takes other values.'}>{describeFilter(filter)}</span>
                  <button aria-label={`Remove filter: ${describeFilter(filter)}`} onClick={() => searchFor(withoutToken(query, filter.token))} type="button">
                    <Icon name="x" size={12} />
                  </button>
                </li>
              ))}
            </ul>
          ) : null}
          {query.trim()
            ? QUICK_FILTERS.filter((quick) => !filters.some((filter) => filter.token.toLowerCase() === quick.token)).map((quick) => (
                <Button icon="plus" key={quick.token} onClick={() => searchFor(withToken(query, quick.token))} size="sm" variant="ghost">
                  {quick.label}
                </Button>
              ))
            : null}
        </div>
        <p className="aw-search__help">
          Narrow with <code>from:@name</code>, <code>in:#channel</code>, <code>during:2026-10</code>, <code>has:link</code>, <code>is:unread</code> or{' '}
          <code>kind:task</code>.
        </p>
        {projectId ? (
          <p className="aw-search__scope">
            <span>
              In <strong>{scope?.name ?? 'one project'}</strong>
            </span>
            <Button icon="x" onClick={searchEverything} size="sm" variant="ghost">
              Search everything
            </Button>
          </p>
        ) : null}
        <div className="aw-search__results" ref={resultsRef}>
          {results.isLoading ? <ListSkeleton label="Searching" rows={3} /> : null}
          {results.isError ? (
            <Banner onRetry={() => void results.refetch()} tone="danger" title="Search didn’t work.">
              {describeLoadError(results.error)}
            </Banner>
          ) : null}
          {results.data && results.data.hits.length === 0 ? (
            <EmptyState
              action={
                projectId ? (
                  <Button onClick={searchEverything} variant="secondary">
                    Search everything
                  </Button>
                ) : null
              }
              icon="search"
              title="Nothing found"
            >
              {filters.length ? 'Try fewer filters or different words.' : 'Try fewer or different words.'}
            </EmptyState>
          ) : null}
          {results.data && results.data.hits.length > 0 ? (
            <RowList label="Results">
              {results.data.hits.map((hit) => {
                const { status, title } = titleAndStatus(hit)
                return (
                  <Row
                    className="aw-row--link"
                    key={`${hit.kind}:${hit.id}`}
                    meta={
                      <span className="aw-search__meta">
                        <StatusChip tone={hit.kind === 'decision' || hit.kind === 'page' ? 'agent' : 'muted'}>{KIND_LABEL[hit.kind]}</StatusChip>
                        {status ? <StatusChip tone="muted">{capitalized(status)}</StatusChip> : null}
                        <span>
                          {/* Searching one project, its name on every row says nothing. */}
                          {[projectId ? null : hit.projectName, hit.conversationName, hit.author].filter(Boolean).join(' · ')}
                          {projectId && !hit.conversationName && !hit.author ? '' : ' · '}
                          <time dateTime={hit.at} title={formatFullDate(hit.at)}>
                            {formatRelative(hit.at)}
                          </time>
                        </span>
                      </span>
                    }
                    title={
                      <Link className="aw-row__link" to={hit.kind === 'message' ? `${hit.path}#message-${hit.id}` : hit.path}>
                        {title}
                      </Link>
                    }
                  >
                    {hit.snippet ? <Snippet text={hit.snippet} /> : null}
                  </Row>
                )
              })}
            </RowList>
          ) : null}
        </div>
      </div>
    </>
  )
}
