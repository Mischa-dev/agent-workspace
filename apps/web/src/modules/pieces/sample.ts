import type { FieldDecl, FieldValue, Thing, ThingDecl } from '../contract'

// Made-up things of any declared kind, for the pieces gallery and its guard
// test: long titles and long values on purpose, so a piece that can't fit
// them shows it.

const TITLES = [
  'Plan the playtest feedback round with the whole team and every agent that helps',
  'Fix the crash on start',
  'Write up what we decided about the save format, and what we tried before it',
  'Release notes',
  'A very long single word: supercalifragilisticexpialidociousness',
  'Check the board on a phone',
]
const STATUSES = ['Backlog', 'In progress', 'Review', 'Done']
const PEOPLE = ['Mischa Nelson', 'Rook', 'Wren Alexandria Montgomery-Smith', 'Mara (agent)']

function sampleValue(field: FieldDecl, index: number): FieldValue {
  switch (field.type) {
    case 'number':
      return (index * 3) % 13
    case 'date':
      return new Date(Date.UTC(2026, 10, 4 + index * 5)).toISOString()
    case 'checkbox':
      return index % 2 === 0
    case 'choice':
      return field.options?.length ? field.options[index % field.options.length] : null
    case 'people':
    case 'labels':
      return index % 3 === 0 ? [] : PEOPLE.slice(0, 1 + (index % 3))
    case 'person':
      return PEOPLE[index % PEOPLE.length]
    case 'url':
      return 'https://example.com/a/rather/long/address/that/must/wrap/somewhere/rather-than-overflow'
    default:
      return index % 4 === 3 ? null : `${field.label} ${index + 1}: details that run long enough to wrap onto a second line`
  }
}

/** `count` sample things of a kind, the same every time. */
export function sampleThings(decl: ThingDecl, module: string, count = 6): Thing[] {
  return Array.from({ length: count }, (_, index) => {
    const id = `sample-${decl.kind}-${index + 1}`
    return {
      id,
      kind: decl.kind,
      module,
      title: TITLES[index % TITLES.length],
      status: STATUSES[index % STATUSES.length],
      owner: index % 5 === 4 ? undefined : { id: `person-${index}`, name: PEOPLE[index % PEOPLE.length] },
      href: '/design-system/pieces',
      fields: Object.fromEntries(decl.fields.map((field) => [field.id, sampleValue(field, index)])),
    }
  })
}
