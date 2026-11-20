import { describe, expect, it } from 'vitest'

import { answerBody, missing, normalizeForm, normalizeQuestions, shownFields } from './formModel'
import { dailyLimitProblem, formFromDraft } from '../collections/formSettings'
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

  it('sends the page’s render token, and the hidden field only when something filled it in', () => {
    const signed = normalizeForm({ titleName: 'Summary', fields: [], renderToken: '17.ab', honeypot: '_aw_homepage' })
    expect(answerBody(signed, { title: 'Hi', _aw_homepage: '' })).toEqual({ title: 'Hi', _aw_rendered: '17.ab' })
    expect(answerBody(signed, { title: 'Hi', _aw_homepage: 'x' })).toEqual({ title: 'Hi', _aw_rendered: '17.ab', _aw_homepage: 'x' })
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
    const settings = formFromDraft({ name: 'F', intro: ' Hi ', thanks: '', asked: { c: 'required', b: 'required', a: undefined }, dailyLimit: '500' }, fields)
    expect(settings).toEqual({ intro: 'Hi', thanks: '', fields: [{ field: 'b' }, { field: 'c', required: true }] })
  })

  it('sends a daily limit only when it differs from the usual 500, and says when it is out of range', () => {
    const draft = { name: 'F', intro: '', thanks: '', asked: {}, dailyLimit: '50' }
    expect(formFromDraft(draft, [])).toEqual({ intro: '', thanks: '', fields: [], dailyLimit: 50 })
    expect(dailyLimitProblem(draft)).toBeNull()
    for (const typed of ['0', '10001', '2.5', '']) expect(dailyLimitProblem({ ...draft, dailyLimit: typed })).not.toBeNull()
  })
})

describe('conditions and sections (T1.28)', () => {
  const asked = normalizeQuestions(
    [
      { id: 'kind', name: 'Kind', type: 'single_choice', required: true, options: [{ id: 'bug', name: 'Bug' }, { id: 'idea', name: 'Idea' }] },
      { id: 'steps', name: 'Steps', type: 'text', required: true, showIf: { field: 'kind', isOneOf: ['Bug'] }, section: { title: 'Details' } },
      { id: 'more', name: 'More', type: 'text', showIf: { field: 'steps' } },
      { id: 'who', name: 'Owner', type: 'person' },
    ],
    'internal',
  )
  const internal = { titleName: 'Name', fields: asked }

  it('keeps people for an internal form, and each question’s condition and heading', () => {
    expect(asked.map((field) => field.id)).toEqual(['kind', 'steps', 'more', 'who'])
    expect(asked[1].section?.title).toBe('Details')
    expect(asked[2].showIf).toEqual({ field: 'steps', isOneOf: [] })
  })

  it('shows a question when an earlier answer names its option, by id or name', () => {
    expect(shownFields(asked, { kind: 'idea' }).map((field) => field.id)).toEqual(['kind', 'who'])
    expect(shownFields(asked, { kind: 'bug' }).map((field) => field.id)).toEqual(['kind', 'steps', 'who'])
    expect(shownFields(asked, { kind: 'bug', steps: 'Open it' }).map((field) => field.id)).toEqual(['kind', 'steps', 'more', 'who'])
  })

  it('asks an internal form’s relation question, with its collection, and a public form never', () => {
    const relation = [{ id: 'parts', name: 'Parts', type: 'relation', required: true, collection: 'c1' }]
    expect(normalizeQuestions(relation, 'internal')).toMatchObject([{ id: 'parts', type: 'relation', collection: 'c1', required: true }])
    expect(normalizeQuestions(relation)).toEqual([])
    const fields = normalizeQuestions(relation, 'internal')
    expect(missing({ titleName: 'Name', fields }, { title: 'A' })).toBe('Fill in Parts.')
    expect(answerBody({ fields }, { title: 'A', parts: ['r1'] })).toEqual({ title: 'A', parts: ['r1'] })
  })

  it('reads an unticked checkbox as No, as the server does', () => {
    const ticked = normalizeQuestions([
      { id: 'agree', name: 'Agree', type: 'checkbox' },
      { id: 'why', name: 'Why not', type: 'text', showIf: { field: 'agree', isOneOf: ['No'] } },
    ])
    expect(shownFields(ticked, {}).map((field) => field.id)).toEqual(['agree'])
    expect(shownFields(ticked, { agree: true }).map((field) => field.id)).toEqual(['agree'])
    expect(shownFields(ticked, { agree: false }).map((field) => field.id)).toEqual(['agree', 'why'])
  })

  it('never asks for a hidden question, and leaves its answer out', () => {
    expect(missing(internal, { title: 'Dark mode', kind: 'idea' })).toBeNull()
    expect(missing(internal, { title: 'Crash', kind: 'bug' })).toBe('Fill in Steps.')
    expect(answerBody(internal, { title: 'Dark mode', kind: 'idea', steps: 'stale', more: 'stale' })).toEqual({ title: 'Dark mode', kind: 'idea' })
  })
})
