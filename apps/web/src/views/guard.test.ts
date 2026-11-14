/// <reference types="node" />
import { readdirSync, readFileSync, statSync } from 'node:fs'
import { join, relative } from 'node:path'

import { describe, expect, it } from 'vitest'

// The freeze on task-side view code (T1.1, guard rail 1 of
// docs/plans/master/plans/T1-fields-views-tasks.md). Tasks and collections
// grew two list screens side by side; from here on views, filters, sorts,
// groupings and boards are built once, in src/views/ (and W1's pieces), and
// tasks get them through the Tasks source. The task list's own files may
// not grow more than 5 % past today's size, and no new file of their kind
// appears outside src/views/ and src/pieces/. The lists only shrink: T1.39
// deletes the old files.

const SRC = join(process.cwd(), 'src')
const HOW =
  'build view, filter, sort, grouping and board features in src/views/ (see docs/plans/collections.md and T1 in docs/plans/master/plans/T1-fields-views-tasks.md)'

/** Today's line counts of the task list's files. */
const FROZEN_LINES: Record<string, number> = {
  'work-items/FilterBar.tsx': 649,
  'work-items/WorkItemBoard.tsx': 786,
  'work-items/WorkItemList.tsx': 548,
  'work-items/filterModel.ts': 281,
  'work-items/boardModel.ts': 317,
  'work-items/viewDraft.ts': 33,
}

/** Files shaped like a second view engine that exist today. */
const ALLOWED_FILES = new Set([
  'collections/RecordBoard.tsx',
  'conversations/threadBoard.tsx',
  'work-items/FilterBar.tsx',
  'work-items/WorkItemBoard.tsx',
  'work-items/filterModel.ts',
  'work-items/viewDraft.ts',
])

/** Files that call the board drag hook today, besides its own. */
const ALLOWED_DRAG = new Set(['collections/RecordBoard.tsx', 'collections/RecordCalendar.tsx', 'work-items/WorkItemBoard.tsx'])

const ENGINE = ['views/', 'pieces/']
const SHAPED = /(Board\.tsx|FilterBar[^/]*|filterModel[^/]*|viewDraft[^/]*)$/

function files(folder: string): string[] {
  return readdirSync(folder).flatMap((name) => {
    const path = join(folder, name)
    if (statSync(path).isDirectory()) return files(path)
    return /\.(ts|tsx)$/.test(name) && !/\.test\.tsx?$/.test(name) ? [path] : []
  })
}

const sources = files(SRC).map((path) => relative(SRC, path).split('\\').join('/'))
const outsideEngine = (name: string) => !ENGINE.some((prefix) => name.startsWith(prefix))

describe('task-side view code is frozen', () => {
  it('keeps the task list files within 5 % of their size', () => {
    const grown = Object.entries(FROZEN_LINES).flatMap(([name, lines]) => {
      let text: string
      try {
        text = readFileSync(join(SRC, name), 'utf8')
      } catch {
        return []
      }
      const now = text.split('\n').length - 1
      return now > Math.floor(lines * 1.05) ? [`${name}: ${now} lines (frozen at ${lines}); ${HOW}`] : []
    })
    expect(grown).toEqual([])
  })

  it('adds no new board, filter bar, filter model or view draft outside src/views/', () => {
    const extra = sources.filter((name) => outsideEngine(name) && SHAPED.test(name) && !ALLOWED_FILES.has(name))
    expect(extra.map((name) => `${name}: ${HOW}`)).toEqual([])
  })

  it('uses the board drag hook only where it is used today, or in src/views/', () => {
    const extra = sources.filter(
      (name) =>
        outsideEngine(name) &&
        name !== 'ui/useBoardDrag.ts' &&
        !ALLOWED_DRAG.has(name) &&
        readFileSync(join(SRC, name), 'utf8').includes('useBoardDrag('),
    )
    expect(extra.map((name) => `${name}: ${HOW}`)).toEqual([])
  })

  it('names only files that still exist, so the lists shrink as files go', () => {
    const gone = [...ALLOWED_FILES, ...ALLOWED_DRAG].filter((name) => !sources.includes(name))
    expect(gone, 'remove these from the guard').toEqual([])
  })
})
