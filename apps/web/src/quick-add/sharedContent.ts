/** What another app shared with Workspace (the Web Share Target's title, text and url). */
export type SharedContent = {
  title: string
  text: string
  url: string
}

const URL_PATTERN = /\bhttps?:\/\/[^\s<>"]+/i
const MAX_TITLE = 240

function clean(value: string | null) {
  return (value ?? '').replace(/\r\n?/g, '\n').trim().slice(0, 20_000)
}

/** Read a share from the page's query; apps put the link in `text` as often as in `url`. */
export function readSharedContent(params: URLSearchParams): SharedContent | null {
  const title = clean(params.get('title'))
  let text = clean(params.get('text'))
  let url = clean(params.get('url'))
  // What to take out of the text: the link as written, with any full stop after it.
  let written = url
  if (!url) {
    written = text.match(URL_PATTERN)?.[0] ?? ''
    url = written.replace(/[.,;:!?)]+$/, '')
  }
  if (url && !/^https?:\/\//i.test(url)) url = ''
  if (url && written) text = text.split(written).join(' ').replace(/[ \t]+/g, ' ').replace(/\s*\n\s*/g, '\n').trim()
  if (text === title) text = ''
  return title || text || url ? { title, text, url } : null
}

/** A task from a share: its title, or the text's first line, and the rest with the link as the description. */
export function taskFromShare(shared: SharedContent) {
  const lines = shared.text.split('\n').filter((line) => line.trim())
  const titleSource = shared.title || lines[0] || hostOf(shared.url) || ''
  const title = [...titleSource].slice(0, MAX_TITLE).join('')
  const rest = shared.title ? lines : lines.slice(1)
  const description = [rest.join('\n'), shared.url].filter(Boolean).join('\n\n')
  return { title, description }
}

/** A message from a share: everything, the link last. */
export function messageFromShare(shared: SharedContent) {
  return [shared.title, shared.text, shared.url].filter(Boolean).join('\n')
}

function hostOf(url: string) {
  try {
    return url ? new URL(url).host : ''
  } catch {
    return ''
  }
}
