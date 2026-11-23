// Share one page (C1.27): with someone in this workspace who isn't in its
// space, or by a link. Either way they read this page and nothing else; a
// public link opens it without signing in.
import { useEffect, useState } from 'react'
import { useQuery, useQueryClient } from '@tanstack/react-query'

import { publicPageUrl } from '../app/href'
import { callOp, newRequestId, ranOrThrow } from '../api/ops'
import { projectLocationHref } from '../projects/locations'
import { Banner, Button, Checkbox, Dialog, SelectField, TextField, describeError, formatDateTime, showToast } from '../ui'
import type { ShareLevel } from './api'

const sharesKey = (pageId: string) => ['page-shares', pageId] as const
const peopleKey = (search: string) => ['page-share-people', search] as const

const LEVELS: { value: ShareLevel; label: string }[] = [
  { value: 'see', label: 'Can read' },
  { value: 'comment', label: 'Can comment' },
  { value: 'edit', label: 'Can edit' },
]

function LevelPicker({ label, onChange, value }: { label: string; onChange: (level: ShareLevel) => void; value: ShareLevel }) {
  return (
    <SelectField aria-label={label} compact onChange={(event) => onChange(event.target.value as ShareLevel)} value={value}>
      {LEVELS.map((level) => (
        <option key={level.value} value={level.value}>
          {level.label}
        </option>
      ))}
    </SelectField>
  )
}

export function SharePageDialog({ onClose, page }: { onClose: () => void; page: { id: string; projectId: string; title: string } }) {
  const queryClient = useQueryClient()
  const shares = useQuery({
    queryKey: sharesKey(page.id),
    queryFn: ({ signal }) => callOp('thing_share_list', { kind: 'page', id: page.id }, { signal }),
  })
  const [query, setQuery] = useState('')
  const [search, setSearch] = useState('')
  useEffect(() => {
    const timer = window.setTimeout(() => setSearch(query.trim()), 200)
    return () => window.clearTimeout(timer)
  }, [query])
  const people = useQuery({
    queryKey: peopleKey(search),
    queryFn: ({ signal }) => callOp('people_search', { query: search, limit: 6 }, { signal }),
    enabled: search.length > 0,
  })
  const [level, setLevel] = useState<ShareLevel>('comment')
  const [publicLink, setPublicLink] = useState(true)
  const [made, setMade] = useState<{ url: string; public: boolean } | null>(null)
  const [busy, setBusy] = useState<string | null>(null)
  const [error, setError] = useState<string | null>(null)

  const changed = (view: Awaited<ReturnType<typeof callOp<'thing_share_list'>>>) => {
    queryClient.setQueryData(sharesKey(page.id), view)
  }
  const run = (key: string, action: () => Promise<void>, failed: string) => {
    setBusy(key)
    setError(null)
    void action()
      .catch((reason: unknown) => setError(`${failed} ${describeError(reason).message}`))
      .finally(() => setBusy(null))
  }
  // A level changed keeps the share's end date.
  const share = (accountId: string, name: string, as: ShareLevel, changing = false, expiresAt?: string | null) =>
    run(
      `share-${accountId}`,
      async () => {
        changed(
          ranOrThrow(
            await callOp('thing_share_add', { kind: 'page', id: page.id, account: accountId, level: as, ...(expiresAt ? { expiresAt } : {}) }, { requestId: newRequestId() }),
            'Sharing it',
          ),
        )
        if (changing) return
        setQuery('')
        showToast({ tone: 'success', title: `Shared with ${name}` })
      },
      changing ? 'It wasn’t changed.' : 'It wasn’t shared.',
    )
  const unshare = (input: { account?: string; link?: string }) =>
    run(
      `remove-${input.account ?? input.link}`,
      async () => changed(ranOrThrow(await callOp('thing_share_remove', { kind: 'page', id: page.id, ...input }, { requestId: newRequestId() }), 'Stopping it')),
      'It wasn’t stopped.',
    )
  const makeLink = () =>
    run(
      'link',
      async () => {
        const link = ranOrThrow(await callOp('thing_link_create', { kind: 'page', id: page.id, public: publicLink }, { requestId: newRequestId() }), 'The link')
        if (link.token) setMade({ url: publicPageUrl(link.token), public: publicLink })
        await queryClient.invalidateQueries({ queryKey: sharesKey(page.id) })
      },
      'The link wasn’t made.',
    )
  const copy = (url: string) => {
    if (!navigator.clipboard) return
    void navigator.clipboard.writeText(url).then(
      () => showToast({ tone: 'success', title: 'Link copied' }),
      () => showToast({ tone: 'danger', title: 'The link wasn’t copied', description: 'Select it and copy it yourself.' }),
    )
  }

  const shared = new Set(shares.data?.accounts.map((account) => account.account))
  const matches = (people.data?.people ?? []).filter((person) => !shared.has(person.id)).slice(0, 6)
  return (
    <Dialog
      description="They get this page and nothing else in its project."
      footer={<Button onClick={onClose}>Done</Button>}
      onClose={onClose}
      open
      title={`Share “${page.title}”`}
    >
      {error ? <Banner tone="danger">{error}</Banner> : null}
      {shares.isError ? <Banner tone="danger">{`The shares didn’t load. ${describeError(shares.error).message}`}</Banner> : null}
      <section aria-labelledby="share-people" className="aw-share">
        <h3 className="aw-share__heading" id="share-people">
          People
        </h3>
        <div className="aw-share__add">
          <TextField label="Add someone from this workspace" onChange={(event) => setQuery(event.target.value)} placeholder="Name or @handle" value={query} />
          <LevelPicker label="What they can do" onChange={setLevel} value={level} />
        </div>
        {search && matches.length ? (
          <ul className="aw-share__list">
            {matches.map((person) => (
              <li className="aw-share__row" key={person.id}>
                <span>
                  {person.name} <span className="aw-share__muted">@{person.handle}</span>
                </span>
                <Button busy={busy === `share-${person.id}`} onClick={() => share(person.id, person.name, level)} size="sm">
                  Share
                </Button>
              </li>
            ))}
          </ul>
        ) : search && people.isSuccess ? (
          <p className="aw-share__muted">Nobody by that name.</p>
        ) : null}
        {shares.data?.accounts.length ? (
          <ul className="aw-share__list">
            {shares.data.accounts.map((account) => (
              <li className="aw-share__row" key={account.account}>
                <span>
                  {account.displayName || 'Someone'}
                  {account.expiresAt ? <span className="aw-share__muted"> · until {formatDateTime(account.expiresAt)}</span> : null}
                </span>
                <span className="aw-share__actions">
                  <LevelPicker
                    label={`What ${account.displayName || 'they'} can do`}
                    onChange={(next) => share(account.account, account.displayName, next, true, account.expiresAt)}
                    value={account.level}
                  />
                  <Button busy={busy === `remove-${account.account}`} onClick={() => unshare({ account: account.account })} size="sm" variant="ghost">
                    Stop sharing
                  </Button>
                </span>
              </li>
            ))}
          </ul>
        ) : shares.isSuccess ? (
          <p className="aw-share__muted">Not shared with anyone on its own.</p>
        ) : null}
        {shares.data?.accounts.length ? (
          <div>
            <Button icon="copy" onClick={() => copy(`${window.location.origin}${projectLocationHref(page.projectId, { kind: 'page', pageId: page.id })}`)} size="sm" variant="ghost">
              Copy the page’s address to send them
            </Button>
          </div>
        ) : null}
      </section>
      <section aria-labelledby="share-links" className="aw-share">
        <h3 className="aw-share__heading" id="share-links">
          Links
        </h3>
        {made ? (
          <div className="aw-share__made">
            <Banner
              action={
                <Button icon="copy" onClick={() => copy(made.url)} size="sm">
                  Copy
                </Button>
              }
              title={made.public ? 'Anyone with this link can read the page.' : 'Anyone signed in with this link can read the page.'}
              tone="info"
            >
              You won’t see it again. It works for 7 days.
            </Banner>
            <code className="aw-share__url">{made.url}</code>
          </div>
        ) : null}
        <Checkbox
          checked={publicLink}
          description="Leave it off and only people signed in here can open it."
          label="Anyone with the link, without signing in"
          onChange={setPublicLink}
        />
        <div>
          <Button busy={busy === 'link'} icon="link" onClick={makeLink}>
            Make a link
          </Button>
        </div>
        {shares.data?.links.length ? (
          <ul className="aw-share__list">
            {shares.data.links.map((link) => (
              <li className="aw-share__row" key={link.id}>
                <span>
                  {link.public ? 'Anyone with the link' : 'Signed-in people with the link'}
                  {link.expiresAt ? <span className="aw-share__muted"> · until {formatDateTime(link.expiresAt)}</span> : null}
                </span>
                <Button busy={busy === `remove-${link.id}`} onClick={() => unshare({ link: link.id })} size="sm" variant="ghost">
                  Turn off
                </Button>
              </li>
            ))}
          </ul>
        ) : shares.isSuccess && !made ? (
          <p className="aw-share__muted">No links yet.</p>
        ) : null}
      </section>
    </Dialog>
  )
}
