import { useState, type FormEvent } from 'react'
import { useQuery } from '@tanstack/react-query'
import { Link, useSearchParams } from 'react-router-dom'

import { requestJson } from '../api/client'
import { useAuthenticatedSession } from '../auth/session'
import { formatFullDate } from '../conversations/timeFormat'
import { AppShell } from '../shell/AppShell'
import { Banner, Button, EmptyState, MessageSkeleton, PageHeader, Row, RowList, StatusChip, TextField } from '../ui'

type SearchHit = {
  kind: 'message' | 'thread' | 'task' | 'page' | 'decision'
  id: string
  title: string
  snippet: string
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
  message: 'Message',
}

/** Search everything you can see: messages, threads, tasks, pages and decisions. */
export function SearchPage() {
  const session = useAuthenticatedSession()
  const [params, setParams] = useSearchParams()
  const query = params.get('q') ?? ''
  const projectId = params.get('projectId') ?? ''
  const [draft, setDraft] = useState(query)
  const results = useQuery({
    queryKey: ['search', query, projectId],
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
  return (
    <AppShell mainClassName="aw-main--page" user={session.user}>
      <div className="aw-page">
        <PageHeader icon="search" title="Search">
          Messages, threads, tasks, pages and decisions you can see. Settled things, like decisions and pages, come first.
        </PageHeader>
        <form className="aw-inline-form" onSubmit={submit} role="search">
          <TextField
            aria-label="Search"
            autoFocus
            id="search-query"
            maxLength={200}
            onChange={(event) => setDraft(event.target.value)}
            placeholder="theming decision, AW-12, “exact phrase”…"
            value={draft}
          />
          <Button disabled={!draft.trim()} icon="search" type="submit" variant="primary">
            Search
          </Button>
        </form>
        {results.isFetching ? <MessageSkeleton label="Searching" rows={2} /> : null}
        {results.isError ? <Banner tone="danger">Search didn’t work. Try different words.</Banner> : null}
        {results.data && results.data.hits.length === 0 ? (
          <EmptyState icon="search" title="Nothing found">
            Try fewer or different words.
          </EmptyState>
        ) : null}
        {results.data && results.data.hits.length > 0 ? (
          <RowList label="Results">
            {results.data.hits.map((hit) => (
              <Row
                key={`${hit.kind}:${hit.id}`}
                meta={
                  <>
                    <StatusChip tone={hit.kind === 'decision' || hit.kind === 'page' ? 'agent' : 'muted'}>{KIND_LABEL[hit.kind]}</StatusChip>
                    <span>
                      {hit.projectName}
                      {hit.conversationName ? ` · ${hit.conversationName}` : ''}
                      {hit.author ? ` · ${hit.author}` : ''} · {formatFullDate(hit.at)}
                    </span>
                  </>
                }
                title={<Link to={hit.kind === 'message' ? `${hit.path}#message-${hit.id}` : hit.path}>{hit.title}</Link>}
              >
                {hit.snippet ? <p className="aw-row__text">{hit.snippet.replaceAll('**', '')}</p> : null}
              </Row>
            ))}
          </RowList>
        ) : null}
      </div>
    </AppShell>
  )
}
