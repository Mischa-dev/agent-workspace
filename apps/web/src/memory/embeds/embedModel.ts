/**
 * What an embed in a page shows, from its path (AWD-90). A collection's view
 * is `/projects/{projectId}/collections/{collectionId}`, with `?view=` for a
 * saved view other than the first; any other path in the app shows as a link
 * to it.
 */
export type EmbedTarget = { kind: 'collection'; projectId: string; collectionId: string; viewId: string | null } | { kind: 'link' }

const COLLECTION = /^\/projects\/([^/]+)\/collections\/([^/]+)\/?$/

export function embedTarget(href: string): EmbedTarget {
  let url: URL
  try {
    url = new URL(href, 'https://app.invalid')
  } catch {
    return { kind: 'link' }
  }
  const match = url.origin === 'https://app.invalid' ? COLLECTION.exec(url.pathname) : null
  if (!match) return { kind: 'link' }
  try {
    return {
      kind: 'collection',
      projectId: decodeURIComponent(match[1]),
      collectionId: decodeURIComponent(match[2]),
      viewId: url.searchParams.get('view') || null,
    }
  } catch {
    return { kind: 'link' }
  }
}

/** The path an embed of a collection's view keeps, and the label it starts with. */
export function collectionEmbed(projectId: string, collection: { id: string; name: string }, view: { id: string; name: string } | null, views: number) {
  const path = `/projects/${encodeURIComponent(projectId)}/collections/${encodeURIComponent(collection.id)}`
  return {
    href: view ? `${path}?view=${encodeURIComponent(view.id)}` : path,
    label: view && views > 1 ? `${collection.name} · ${view.name}` : collection.name,
  }
}
