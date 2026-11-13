import { describe, expect, it } from 'vitest'

import { answerBody, missing, normalizeForm } from './formModel'
import { formFromDraft } from '../collections/formSettings'
import type { FieldDefinition } from '../fields/fieldModel'

const form = normalizeForm({
  name: 'Requests',
  collection: 'Requests',
  titleName: 'Summary',
  fields: [
    { id: 'sev', name: 'Severity', type: 'single_choice', required: true, options: [{ id: 'hi', name: 'High' }] },
    { id: 'n', name: 'Count', type: 'number' },
    { id: 'ok', name: 'Agreed', type: 'checkbox', required: true },
    { id: 'who', name: 'Owner', type: 'person' },
  ],
})

describe('public form', () => {
  it('drops field types a form cannot ask', () => {
    expect(form.fields.map((field) => field.id)).toEqual(['sev', 'n', 'ok'])
  })

  it('asks for the title, then each required field, but never a checkbox', () => {
    expect(missing(form, {})).toBe('Fill in Summary.')
    expect(missing(form, { title: 'Broken' })).toBe('Fill in Severity.')
    expect(missing(form, { title: 'Broken', sev: 'hi' })).toBeNull()
  })

  it('sends numbers as numbers and leaves blanks out', () => {
    expect(answerBody(form, { title: ' Broken ', sev: 'hi', n: '3', ok: true })).toEqual({ title: 'Broken', sev: 'hi', n: 3, ok: true })
    expect(answerBody(form, { title: 'Broken', n: '  ' })).toEqual({ title: 'Broken' })
  })
})

describe('form settings', () => {
  const field = (id: string, type: FieldDefinition['type']): FieldDefinition => ({
    id,
    name: id,
    description: '',
    type,
    options: [],
    target: null,
    position: 1,
    archived: false,
    version: 1,
  })

  it('keeps the collection’s order and never requires a checkbox', () => {
    const fields = [field('a', 'text'), field('b', 'checkbox'), field('c', 'date')]
    const settings = formFromDraft({ name: 'F', intro: ' Hi ', thanks: '', asked: { c: 'required', b: 'required', a: undefined } }, fields)
    expect(settings).toEqual({ intro: 'Hi', thanks: '', fields: [{ field: 'b' }, { field: 'c', required: true }] })
  })
})
