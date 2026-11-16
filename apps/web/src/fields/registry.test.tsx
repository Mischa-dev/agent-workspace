import { cleanup, fireEvent, screen, waitFor } from '@testing-library/react'
import { afterEach, describe, expect, it, onTestFinished, vi } from 'vitest'

import { axeViolations, renderPiece, WIDTHS, type Width } from '../pieces/testing'
import { FieldEditor } from './FieldEditor'
import { DECLARED_THINGS } from '../pieces/declared'
import { definitionOfDecl, type FieldDefinition, type FieldReference, type FieldValue } from './fieldModel'
import serverTypes from './fieldTypes.generated.json'
import { FIELD_TYPE_LIST, type FieldType } from './fieldTypes'
import { FieldValueView } from './FieldValueView'
import { fieldSpec, registeredTypes } from './registry'

// The field registry (T1.13a): every type the server has is registered, and
// every type draws a value and (where people fill it in) edits one, at a
// desk's width and a phone's, with no accessibility problems axe can find.

vi.mock('../work-items/hooks', () => ({
  useAssignableActors: () => ({ data: [{ id: 'p1', displayName: 'Ana Lee', handle: 'ana', kind: 'human' }], isFetching: false, isError: false }),
}))
vi.mock('../work-items/planningHooks', () => ({
  useTaskSearch: () => ({ data: [{ id: 't1', key: 'AWD-1', title: 'Fix the login', statusCategory: 'started' }], isFetching: false, isError: false }),
}))
vi.mock('../memory/api', () => ({
  memoryKeys: { pages: (projectId: string) => ['project', projectId, 'pages'] },
  listPages: async () => ({ pages: [{ id: 'pg1', title: 'Launch plan' }] }),
}))
vi.mock('../api/client', async (importOriginal) => ({
  ...(await importOriginal<typeof import('../api/client')>()),
  requestJson: async () => ({ records: [{ id: 'r1', title: 'Crash on start', number: 3 }] }),
}))

afterEach(() => {
  cleanup()
  vi.unstubAllGlobals()
})

const options = [
  { id: 'o1', name: 'Low', color: '#22c55e', archived: false, category: 'unstarted' as const },
  { id: 'o2', name: 'High', color: '#ef4444', archived: false, category: 'started' as const },
]

const references: FieldReference[] = [
  { id: 'p1', kind: 'person', label: 'Ana Lee', key: '@ana' },
  { id: 't1', kind: 'task', label: 'Fix the login', key: 'AWD-1' },
]

function fieldOf(type: FieldType, extra: Partial<FieldDefinition> = {}): FieldDefinition {
  return {
    id: `f-${type}`,
    name: `Sample ${type.replace(/_/g, ' ')}`,
    description: '',
    type,
    options: type === 'single_choice' || type === 'multiple_choice' || type === 'status' ? options : [],
    target: type === 'link' ? 'task' : null,
    position: 1,
    archived: false,
    version: 1,
    ...extra,
  }
}

/** A value of each type, as the server sends it. */
const SAMPLES: Record<FieldType, FieldValue> = {
  text: 'Hello there',
  long_text: 'First line\nSecond line',
  number: 2.5,
  date: '2026-11-14',
  single_choice: 'o1',
  multiple_choice: ['o1', 'o2'],
  status: 'o2',
  person: 'p1',
  url: 'https://example.com/docs',
  email: 'ada@example.com',
  phone: '+44 20 7946 0958',
  checkbox: true,
  link: 't1',
  created_time: '2026-11-14 10:00:00+00',
  edited_time: '2026-11-15 09:30:00+00',
  created_by: 'p1',
  edited_by: 'p1',
  auto_id: 7,
  people: ['Ana Lee'],
  labels: ['Bug'],
}

/** How to change a value of each type in its editor, and what that sets. */
type Edit = { act: () => Promise<void> | void; expected: FieldValue | null }

const typeAndSet = (text: string, label: string): (() => void) => () => {
  fireEvent.change(screen.getByLabelText(label, { selector: 'input, textarea' }), { target: { value: text } })
  fireEvent.click(screen.getByRole('button', { name: 'Set' }))
}
const pickOption = (name: RegExp | string) => async () => {
  fireEvent.click(await screen.findByRole('option', { name }))
}

function editOf(field: FieldDefinition): Edit | null {
  switch (field.type) {
    case 'text':
    case 'long_text':
      return { act: typeAndSet('Written', field.name), expected: 'Written' }
    case 'number':
      return { act: typeAndSet('1,250.5', field.name), expected: 1250.5 }
    case 'url':
      return { act: typeAndSet('https://example.org', field.name), expected: 'https://example.org' }
    case 'email':
      return { act: typeAndSet('bo@example.org', field.name), expected: 'bo@example.org' }
    case 'phone':
      return { act: typeAndSet('+1 555 0100', field.name), expected: '+1 555 0100' }
    case 'date':
      return {
        act: () => {
          fireEvent.click(screen.getAllByRole('gridcell')[10].querySelector('button')!)
        },
        expected: expect.stringMatching(/^\d{4}-\d{2}-\d{2}$/) as unknown as string,
      }
    case 'single_choice':
      return { act: pickOption(/High/), expected: 'o2' }
    case 'status':
      return { act: pickOption(/Low/), expected: 'o1' }
    case 'multiple_choice':
      return { act: pickOption(/Low/), expected: ['o2'] }
    case 'person':
      return { act: pickOption(/Ana Lee/), expected: 'p1' }
    case 'checkbox':
      return { act: pickOption('Not checked'), expected: null }
    case 'link':
      return field.target === 'page'
        ? { act: pickOption(/Launch plan/), expected: 'pg1' }
        : field.target === 'record'
          ? { act: pickOption(/Crash on start/), expected: 'r1' }
          : { act: pickOption(/Fix the login/), expected: 't1' }
    default:
      return null
  }
}

const widths = Object.keys(WIDTHS) as Width[]

describe('the field registry', () => {
  it('registers every field type the server has, and only those', () => {
    const server = serverTypes.map((entry) => entry.type).sort()
    expect([...FIELD_TYPE_LIST].sort()).toEqual(server)
    for (const type of server) {
      expect(registeredTypes(), `${type} is not in the field registry`).toContain(type)
      const spec = fieldSpec(type)
      expect(spec.type).toBe(type)
      expect(spec.label).toBeTruthy()
      expect(spec.width).toBeGreaterThan(0)
      expect(spec.ops.length).toBeGreaterThan(0)
    }
  })

  it('agrees with the server on which types store values and which read the record', () => {
    for (const entry of serverTypes) {
      const spec = fieldSpec(entry.type)
      expect(spec.system, entry.type).toBe(entry.system)
      // Types nobody fills in have no editor; stored, non-system types do.
      expect(spec.Editor === null, entry.type).toBe(entry.system || !entry.stored)
    }
  })

  const cases = FIELD_TYPE_LIST.flatMap((type) => widths.map((width) => [type, width] as const))

  it.each(cases)('draws a %s value at %s width', async (type, width) => {
    const field = fieldOf(type)
    const { container } = renderPiece(
      <div>
        <FieldValueView field={field} references={references} value={SAMPLES[type]} />
      </div>,
      { width },
    )
    expect(container.textContent?.length || container.querySelector('svg')).toBeTruthy()
    expect(await axeViolations(container)).toEqual([])
  })

  const editable = [
    ...FIELD_TYPE_LIST.filter((type) => fieldSpec(type).Editor !== null).map((type) => fieldOf(type)),
    fieldOf('link', { id: 'f-page', name: 'Sample page', target: 'page' }),
    fieldOf('link', { id: 'f-record', name: 'Sample record', target: 'record', collection: 'c1' }),
  ]
  const editCases = editable.flatMap((field) => widths.map((width) => [`${field.type}${field.target ? ` (${field.target})` : ''}`, width, field] as const))

  it.each(editCases)('edits a %s value at %s width', async (_, width, field) => {
    const anchor = document.createElement('button')
    anchor.textContent = field.name
    document.body.append(anchor)
    onTestFinished(() => anchor.remove())
    const onSet = vi.fn()
    const { baseElement } = renderPiece(
      <FieldEditor anchor={anchor} field={field} onClose={() => undefined} onSet={onSet} projectId="project-1" references={references} value={SAMPLES[field.type]} />,
      { width },
    )
    const edit = editOf(field)
    expect(edit, `${field.type} has an editor but the test doesn't know how to use it`).not.toBeNull()
    await edit!.act()
    await waitFor(() => expect(onSet).toHaveBeenCalledWith(edit!.expected))
    expect(await axeViolations(baseElement)).toEqual([])
  })

  it('draws the fields module manifests declare, as definitions', async () => {
    const declared = DECLARED_THINGS.flatMap(({ decl }) => decl.fields)
    expect(declared.length).toBeGreaterThan(0)
    for (const [index, decl] of declared.entries()) {
      const field = definitionOfDecl(decl, index + 1)
      expect(registeredTypes()).toContain(field.type)
      expect(field.name).toBe(decl.label)
      expect(field.options.map((option) => option.name)).toEqual(decl.options ?? [])
    }
    const choice = declared.find((decl) => decl.type === 'single_choice' && decl.options?.length)
    if (choice) {
      const { container } = renderPiece(<FieldValueView field={definitionOfDecl(choice)} value={choice.options![0]} />)
      expect(container.textContent).toContain(choice.options![0])
      expect(await axeViolations(container)).toEqual([])
    }
  })

  it('reads a type it doesn’t know as text, without an editor', () => {
    const spec = fieldSpec('hologram')
    expect(spec.type).toBe('text')
    expect(spec.Editor).toBeNull()
  })
})
