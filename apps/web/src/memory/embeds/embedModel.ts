/**
 * What an embed in a page shows, from its path (AWD-90). A collection's view
 * is `/projects/{projectId}/collections/{collectionId}`, with `?view=` for a
 * saved view other than the first. A sheet's range (AWD-54) is
 * `/projects/{projectId}/sheets/{sheetId}`, with `?tab=` and `?range=` (such
 * as `A1:D9`). A file of the page (C1.33), such as a pasted picture, is
 * `/files/{fileId}`. Any other path in the app shows as a link to it.
 */
export type EmbedTarget =
  | { kind: 'collection'; projectId: string; collectionId: string; viewId: string | null }
  | { kind: 'sheet'; projectId: string; sheetId: string; tabId: string | null; range: string | null; name: string | null }
  | { kind: 'file'; fileId: string }
  | { kind: 'link' }

const FILE = /^\/files\/([0-9a-f-]{36})\/?$/i
const COLLECTION = /^\/projects\/([^/]+)\/collections\/([^/]+)\/?$/
const SHEET = /^\/projects\/([^/]+)\/sheets\/([^/]+)\/?$/

export function embedTarget(href: string): EmbedTarget {
  let url: URL
  try {
    url = new URL(href, 'https://app.invalid')
  } catch {
    return { kind: 'link' }
  }
  if (url.origin !== 'https://app.invalid') return { kind: 'link' }
  try {
    const collection = COLLECTION.exec(url.pathname)
    if (collection) {
      return {
        kind: 'collection',
        projectId: decodeURIComponent(collection[1]),
        collectionId: decodeURIComponent(collection[2]),
        viewId: url.searchParams.get('view') || null,
      }
    }
    const file = FILE.exec(url.pathname)
    if (file) return { kind: 'file', fileId: file[1].toLowerCase() }
    const sheet = SHEET.exec(url.pathname)
    if (sheet) {
      return {
        kind: 'sheet',
        projectId: decodeURIComponent(sheet[1]),
        sheetId: decodeURIComponent(sheet[2]),
        tabId: url.searchParams.get('tab') || null,
        range: url.searchParams.get('range') || null,
        name: url.searchParams.get('name') || null,
      }
    }
  } catch {
    return { kind: 'link' }
  }
  return { kind: 'link' }
}

/** The path an embed of a collection's view keeps, and the label it starts with. */
export function collectionEmbed(projectId: string, collection: { id: string; name: string }, view: { id: string; name: string } | null, views: number) {
  const path = `/projects/${encodeURIComponent(projectId)}/collections/${encodeURIComponent(collection.id)}`
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
  const path = `/projects/${encodeURIComponent(projectId)}/sheets/${encodeURIComponent(sheet.id)}`
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
