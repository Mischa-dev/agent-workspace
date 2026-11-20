import { createElement, lazy, Suspense } from 'react'
import { Link } from 'react-router-dom'

import { embedView, registeredEmbed } from '../../modules/embeds'
import { Icon, ListSkeleton } from '../../ui'
import { embedTarget } from './embedModel'
import './embeds.css'

const FileEmbed = lazy(() => import('../../files/FileEmbed'))
// Anything else a module's manifest embeds (a task, a task list, a
// whiteboard), or a link to it.
const ModuleEmbed = lazy(() => import('./ModuleEmbed'))

/**
 * Something of the app's shown live in a page (an embed block): what a
 * module registers for its path (M1.5: a collection's view, a sheet's
 * range), a file of the page, or else a link to the path. `framed` draws its
 * box; the page editor keeps the box itself, around the block it can select.
 */
export function PageEmbed({ framed = true, href, label }: { framed?: boolean; href: string; label: string }) {
  const registered = registeredEmbed(href)
  const target = registered ? null : embedTarget(href)
  let content
  if (registered) {
    // One lazy view per registration, made once and kept (embedView).
    content = (
      <Suspense fallback={<ListSkeleton label={`Loading ${label || registered.registration.loading}`} rows={3} />}>
        {createElement(embedView(registered.module, registered.registration), { label, params: registered.params })}
      </Suspense>
    )
  } else if (target?.kind === 'file') {
    content = (
      <Suspense fallback={<ListSkeleton label={`Loading ${label || 'the file'}`} rows={2} />}>
        <FileEmbed fileId={target.fileId} label={label} />
      </Suspense>
    )
  } else {
    content = (
      <Suspense
        fallback={
          <Link className="aw-embed__link" to={href}>
            <Icon name="link" size={16} />
            <span>{label || href}</span>
          </Link>
        }
      >
        <ModuleEmbed href={href} label={label} />
      </Suspense>
    )
  }
  return framed ? <div className="aw-embed">{content}</div> : content
}
