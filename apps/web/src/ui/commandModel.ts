import type { ReactNode } from 'react'

import type { IconName } from './icons'

export type CommandItem = {
  id: string
  label: string
  /** The heading it's listed under, such as "Go to" or "Tasks". */
  group: string
  /** Shown after the label in a quieter style: a key, a project, a path. */
  hint?: string
  icon?: IconName
  /** Leading content in place of the icon, such as a Project tile or an avatar. */
  mark?: ReactNode
  /** Extra words it's found by. */
  keywords?: string[]
  shortcut?: string
  /** Already matched by the server (a search result): listed without local filtering. */
  matched?: boolean
  /** AWD-75: where it leads, when that isn't the open project or Home: the project, and the module of the page (otherwise its id's). */
  project?: string
  module?: string
  onSelect: () => void
}

function normalize(value: string) {
  return value.toLocaleLowerCase().normalize('NFKD').replace(/[̀-ͯ]/g, '')
}

/** How well `query` matches `text`: 0 means not at all. */
export function matchScore(text: string, query: string): number {
  const haystack = normalize(text)
  const needle = normalize(query.trim())
  if (!needle) return 1
  if (haystack === needle) return 100
  if (haystack.startsWith(needle)) return 80
  const wordStart = haystack.split(/[\s/#@._-]+/).some((word) => word.startsWith(needle))
  if (wordStart) return 60
  if (haystack.includes(needle)) return 40
  // Letters in order, like "gs" for "Go to settings".
  let index = 0
  for (const character of haystack) {
    if (character === needle[index]) index += 1
    if (index === needle.length) return 10
  }
  return 0
}

/** The items that match, best first, keeping each group's own order for ties. */
export function filterCommands(items: CommandItem[], query: string): CommandItem[] {
  if (!query.trim()) return items
  return items
    .map((item, order) => {
      const score = item.matched
        ? 50
        : Math.max(
            matchScore(item.label, query),
            ...(item.keywords ?? []).map((keyword) => matchScore(keyword, query) * 0.9),
            item.hint ? matchScore(item.hint, query) * 0.8 : 0,
          )
      return { item, order, score }
    })
    .filter((candidate) => candidate.score > 0)
    .sort((left, right) => right.score - left.score || left.order - right.order)
    .map((candidate) => candidate.item)
}
