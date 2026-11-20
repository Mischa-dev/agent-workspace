import { legacyPathOf, spaceHref } from '../../app/href'

/**
 * What an embed in a page shows, from its path (either address shape). A
 * file of the page (C1.33), such as a pasted picture, is `/files/{fileId}`.
 * Modules read their own
 * paths through their embed registrations (M1.5: a collection's view, a
 * sheet's range); any other path in the app shows as a link to it. The
 * paths a collection's or sheet's picker keeps are built here.
 */
export type EmbedTarget = { kind: 'file'; fileId: string } | { kind: 'link' }

const FILE = /^\/files\/([0-9a-f-]{36})\/?$/i

export function embedTarget(href: string): EmbedTarget {
  let url: URL
  try {
    url = new URL(href, 'https://app.invalid')
  } catch {
    return { kind: 'link' }
  }
  if (url.origin !== 'https://app.invalid') return { kind: 'link' }
  // Either shape of address: /w/<ws>/s/<space>/... or the old /projects/<space>/...
  const path = legacyPathOf(url.pathname)
  try {
    const file = FILE.exec(path)
    if (file) return { kind: 'file', fileId: file[1].toLowerCase() }
  } catch {
    return { kind: 'link' }
  }
  return { kind: 'link' }
}

/** The path an embed of a collection's view keeps, and the label it starts with. */
export function collectionEmbed(projectId: string, collection: { id: string; name: string }, view: { id: string; name: string } | null, views: number) {
  const path = spaceHref(projectId, `/collections/${encodeURIComponent(collection.id)}`)
  return {
    href: view ? `${path}?view=${encodeURIComponent(view.id)}` : path,
    label: view && views > 1 ? `${collection.name} · ${view.name}` : collection.name,
  }
}

/**
 * The path an embed of a sheet's tab (and range, if one) keeps, and its
 * label. A named range (C1.20) is kept by its name, so the embed follows it
 * when it moves.
 */
export function sheetEmbed(projectId: string, sheet: { id: string; name: string }, tab: { id: string; name: string } | null, tabs: number, range: string | null, name: string | null = null) {
  const path = spaceHref(projectId, `/sheets/${encodeURIComponent(sheet.id)}`)
  const query = new URLSearchParams()
  if (name) query.set('name', name)
  else {
    if (tab) query.set('tab', tab.id)
    if (range) query.set('range', range)
  }
  const search = query.toString()
  const where = name ?? [tab && tabs > 1 ? tab.name : '', range ?? ''].filter(Boolean).join(' ')
  return {
    href: search ? `${path}?${search}` : path,
    label: where ? `${sheet.name} · ${where}` : sheet.name,
  }
}

/** The path a file of the page keeps, and its label (the file's name). */
export function fileEmbed(file: { id: string; name: string }) {
  return { href: `/files/${encodeURIComponent(file.id)}`, label: file.name }
}
