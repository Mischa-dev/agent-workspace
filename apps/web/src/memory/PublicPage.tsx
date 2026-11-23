// A page opened by a public link (C1.27): its words, with no app around it
// and no sign-in. A link that stopped working says so.
import { useEffect, useState } from 'react'
import { useParams } from 'react-router-dom'

import { apiUrl } from '../api/client'
import { BrandMark } from '../auth/AuthLayout'
import { formatFullDate, formatRelative } from '../ui'
import { Markdown } from '../ui/markdown'
import './publicPage.css'

type SharedPage = { id: string; title: string; body: string; updatedAt: string }
type Loaded = { state: 'loading' } | { state: 'ready'; page: SharedPage } | { state: 'gone' }

export function PublicPage() {
  const { token = '' } = useParams()
  const [loaded, setLoaded] = useState<Loaded>({ state: 'loading' })
  useEffect(() => {
    let live = true
    const read = async (path: string, credentials: RequestCredentials) => {
      const response = await fetch(apiUrl(path), { credentials })
      if (!response.ok) throw new Error(String(response.status))
      return (await response.json()) as SharedPage
    }
    const named = encodeURIComponent(token)
    // A public link opens without signing in; a link for people signed in
    // here opens with the reader's session.
    void read(`/api/v1/public/pages/${named}`, 'omit')
      .catch(() => read(`/api/v1/shared/${named}`, 'include'))
      .then(
        (page) => {
          if (!live) return
          setLoaded({ state: 'ready', page })
          document.title = page.title
        },
        () => {
          if (live) setLoaded({ state: 'gone' })
        },
      )
    return () => {
      live = false
    }
  }, [token])

  return (
    <main className="aw-ppage">
      <article className="aw-ppage__card">
        {loaded.state === 'loading' ? (
          <p aria-busy="true" className="aw-ppage__quiet">
            Opening the page…
          </p>
        ) : loaded.state === 'gone' ? (
          <>
            <h1 className="aw-ppage__title">This link doesn’t work</h1>
            <p className="aw-ppage__quiet">
              It may have expired or been turned off, or it opens only for people signed in here. Ask whoever sent it for a new one, or{' '}
              <a href={`/sign-in?returnTo=${encodeURIComponent(`/p/${token}`)}`}>sign in</a>.
            </p>
          </>
        ) : (
          <>
            <h1 className="aw-ppage__title">{loaded.page.title}</h1>
            <p className="aw-ppage__quiet">
              Updated{' '}
              <time dateTime={loaded.page.updatedAt} title={formatFullDate(loaded.page.updatedAt)}>
                {formatRelative(loaded.page.updatedAt)}
              </time>
            </p>
            <Markdown className="aw-doc" empty="Nothing here yet." source={withoutTitle(loaded.page.body, loaded.page.title)} />
          </>
        )}
      </article>
      <p className="aw-ppage__brand">
        <BrandMark size={18} />
        <span>Shared from Agent Workspace</span>
      </p>
    </main>
  )
}

/** A body that opens with the page's own title as a heading doesn't say it twice. */
function withoutTitle(body: string, title: string) {
  const match = /^\s*#\s+(.+?)\s*#*\s*(?:\n|$)/.exec(body)
  return match && match[1].trim() === title.trim() ? body.slice(match[0].length).replace(/^\s*\n/, '') : body
}

export default PublicPage
