import { spaceHref } from '../../app/href'

/** An address pasted from this app, as the path an embed keeps. */
export function embedPath(address: string, origin = window.location.origin): string | null {
  const text = address.trim()
  if (!text) return null
  try {
    const url = new URL(text, origin)
    return url.origin === origin ? `${url.pathname}${url.search}` : null
  } catch {
    return null
  }
}

// The paths the modules' pickers keep. Which module draws a path is its
// embed registration's (M1.5, W1.26); see `modules/embeds.ts`.

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

/** The path an embed of a sheet's chart keeps (C1.18), and its label. */
export function sheetChartEmbed(projectId: string, sheet: { id: string; name: string }, chart: { id: string; title: string }) {
  const path = spaceHref(projectId, `/sheets/${encodeURIComponent(sheet.id)}`)
  return { href: `${path}?chart=${encodeURIComponent(chart.id)}`, label: `${sheet.name} · ${chart.title}` }
}

/** The path a file of the page keeps, and its label (the file's name). */
export function fileEmbed(file: { id: string; name: string }) {
  return { href: `/files/${encodeURIComponent(file.id)}`, label: file.name }
}
