import { createElement, Suspense } from 'react'
import { Link } from 'react-router-dom'

import { embedView, registeredEmbed } from '../../modules/embeds'
import { Icon, ListSkeleton } from '../../ui'
import './embeds.css'

/**
 * Something of the app's shown live in a page (an embed block): what a
 * module registers for its path (W1.26: a task, a task list, a page, a
 * whiteboard, a thread, a collection's view, a sheet's range, a file of the
 * page), or else a link to the path. `framed` draws its box; the page editor
 * keeps the box itself, around the block it can select.
 */
export function PageEmbed({ framed = true, href, label }: { framed?: boolean; href: string; label: string }) {
  const registered = registeredEmbed(href)
  const content = registered ? (
    // One lazy view per registration, made once and kept (embedView).
    <Suspense fallback={<ListSkeleton label={`Loading ${label || registered.registration.loading}`} rows={3} />}>
      {createElement(embedView(registered.module, registered.registration), { href, label, params: registered.params })}
    </Suspense>
  ) : (
    <Link className="aw-embed__link" to={href}>
      <Icon name="link" size={16} />
      <span>{label || href}</span>
    </Link>
  )
  return framed ? <div className="aw-embed">{content}</div> : content
}
