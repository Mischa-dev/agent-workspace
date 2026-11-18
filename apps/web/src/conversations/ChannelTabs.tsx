import { useState } from 'react'
import { useQuery, useQueryClient } from '@tanstack/react-query'

import { isApiError } from '../api/errors'
import { callOp } from '../api/ops'
import { queryKeys } from '../api/queryKeys'
import { toastError } from '../app/mutations'
import { createPage } from '../memory/api'
import { IconButton, showToast } from '../ui'

/**
 * A channel's tabs (C1.26): its messages, then each page shown on it, and
 * for people who can post, a way to add a page. A page the viewer can't
 * open has no tab.
 */
export function ChannelTabs({
  canAdd,
  channelId,
  channelName,
  onSelect,
  projectId,
  selected,
}: {
  canAdd: boolean
  channelId: string
  channelName: string
  onSelect: (pageId: string | null) => void
  projectId: string
  selected: string | null
}) {
  const queryClient = useQueryClient()
  const key = queryKeys.conversationTabs(projectId, channelId)
  const tabs = useQuery({
    queryKey: key,
    queryFn: () => callOp('conversation_tab_list', { conversationId: channelId }, { retries: 0 }),
  })
  const [busy, setBusy] = useState(false)
  const items = tabs.data?.tabs ?? []
  if (!canAdd && items.length === 0) return null

  const set = (pageId: string, remove: boolean) =>
    callOp('conversation_tab_set', { conversationId: channelId, page: pageId, remove }).then((result) => {
      queryClient.setQueryData(key, result)
      return result
    })
  const add = () => {
    setBusy(true)
    void newTabPage(projectId, channelName)
      .then((page) => set(page.id, false).then(() => onSelect(page.id)))
      .catch((reason: unknown) => toastError('The page tab wasn’t added', reason))
      .finally(() => setBusy(false))
  }
  const remove = (pageId: string, title: string) => {
    void set(pageId, true)
      .then(() => {
        if (selected === pageId) onSelect(null)
        showToast({ title: `“${title}” is no longer a tab`, onUndo: () => void set(pageId, false).catch(() => undefined) })
      })
      .catch((reason: unknown) => toastError('The tab wasn’t removed', reason))
  }

  return (
    <nav aria-label={`Tabs in #${channelName}`} className="aw-chtabs">
      <button aria-current={selected === null ? 'page' : undefined} className="aw-chtabs__tab" onClick={() => onSelect(null)} type="button">
        Messages
      </button>
      {items.map((tab) => (
        <span className="aw-chtabs__item" key={tab.id}>
          <button
            aria-current={selected === tab.id ? 'page' : undefined}
            className="aw-chtabs__tab"
            onClick={() => onSelect(tab.id)}
            type="button"
          >
            {tab.title}
          </button>
          {canAdd && selected === tab.id ? (
            <IconButton icon="x" label={`Remove the ${tab.title} tab`} onClick={() => remove(tab.id, tab.title)} size="sm" />
          ) : null}
        </span>
      ))}
      {canAdd && items.length < 20 ? <IconButton disabled={busy} icon="plus" label="Add a page tab" onClick={add} size="sm" /> : null}
    </nav>
  )
}

/**
 * A new page for a tab, titled after the channel: a space's pages have
 * distinct titles, so a second tab is "name 2", then "name 3".
 */
async function newTabPage(projectId: string, channelName: string) {
  for (let attempt = 1; ; attempt += 1) {
    try {
      return await createPage(projectId, { title: attempt === 1 ? channelName : `${channelName} ${attempt}` })
    } catch (reason) {
      if (!(isApiError(reason) && reason.status === 409) || attempt >= 20) throw reason
    }
  }
}
