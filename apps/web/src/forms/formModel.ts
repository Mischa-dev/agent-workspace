/**
 * A collection's public form (AWD-110), as `/forms/{token}` reads it: the
 * collection's name and the fields the form asks for, nothing else. Answers
 * go to the link's report URL like any bug report, and each one becomes a
 * record, made as the person who shared the form.
 */

import { apiUrl } from '../api/client'

export type PublicFieldType = 'text' | 'long_text' | 'number' | 'date' | 'single_choice' | 'multiple_choice' | 'status' | 'url' | 'email' | 'phone' | 'checkbox'
/** What an internal form (T1.28) asks too: the space's people. */
export type QuestionType = PublicFieldType | 'person' | 'people'

/** A question shows only when an earlier answer is one of these values (option names; Yes or No for a checkbox), or, with none, is given at all. */
export type ShowIf = { field: string; isOneOf: string[] }

export type PublicField = {
  id: string
  name: string
  description: string
  type: QuestionType
  required: boolean
  options: { id: string; name: string; color: string }[]
  showIf?: ShowIf
  /** A heading this question starts. */
  section?: { title: string; description: string }
}

export type PublicForm = {
  name: string
  collection: string
  titleName: string
  intro: string
  thanks: string
  fields: PublicField[]
  /** Sent back with the answers, to show the page was loaded first. */
  renderToken: string
  /** A field people never see; a script that fills it in is ignored. */
  honeypot: string
}

/** Answers sent sooner than this after the form loaded are refused, so the page waits it out. */
export const MIN_RENDER_AGE_MS = 3_000

/** What the person typed or picked, by field ID; the title under "title". */
export type Answers = Record<string, string | string[] | boolean>

type Wire = Record<string, unknown>
const asRecord = (value: unknown): Wire => (value && typeof value === 'object' && !Array.isArray(value) ? (value as Wire) : {})
const asString = (value: unknown) => (typeof value === 'string' ? value : '')
const TYPES: readonly PublicFieldType[] = ['text', 'long_text', 'number', 'date', 'single_choice', 'multiple_choice', 'status', 'url', 'email', 'phone', 'checkbox']
const INTERNAL_TYPES: readonly QuestionType[] = [...TYPES, 'person', 'people']

/** The questions as the server lists them, keeping only the types this audience is asked. */
export function normalizeQuestions(value: unknown, audience: 'public' | 'internal' = 'public'): PublicField[] {
  const types = audience === 'internal' ? INTERNAL_TYPES : TYPES
  return (Array.isArray(value) ? value : []).flatMap((entry) => {
    const field = asRecord(entry)
    const type = types.find((candidate) => candidate === field.type)
    if (!type || typeof field.id !== 'string') return []
    const condition = asRecord(field.showIf)
    const section = asRecord(field.section)
    return [
      {
        id: field.id,
        name: asString(field.name),
        description: asString(field.description),
        type,
        required: field.required === true,
        options: (Array.isArray(field.options) ? field.options : []).flatMap((option) => {
          const parts = asRecord(option)
          return typeof parts.id === 'string' ? [{ id: parts.id, name: asString(parts.name), color: asString(parts.color) }] : []
        }),
        ...(typeof condition.field === 'string'
          ? { showIf: { field: condition.field, isOneOf: (Array.isArray(condition.isOneOf) ? condition.isOneOf : []).filter((word): word is string => typeof word === 'string') } }
          : {}),
        ...(asString(section.title) ? { section: { title: asString(section.title), description: asString(section.description) } } : {}),
      },
    ]
  })
}

/** The words an answer gives a condition: options by name, a checkbox as Yes or No. */
function answerWords(field: PublicField, value: Answers[string] | undefined): string[] {
  if (value === undefined) return []
  // As the server reads them: an explicit No is an answer too.
  if (typeof value === 'boolean') return value ? ['yes', 'true'] : ['no', 'false']
  const values = Array.isArray(value) ? value : [value]
  return values
    .map((entry) => entry.trim())
    .filter(Boolean)
    .map((entry) => (field.options.find((option) => option.id === entry)?.name ?? entry).toLowerCase())
}

/** The questions showing for these answers: one whose condition isn't met, or that reads a hidden one, is left out (as the server leaves out its answer). */
export function shownFields(fields: readonly PublicField[], answers: Answers): PublicField[] {
  const shown: PublicField[] = []
  for (const field of fields) {
    if (field.showIf) {
      const { field: on, isOneOf } = field.showIf
      const earlier = shown.find((candidate) => candidate.id === on)
      const words = earlier ? answerWords(earlier, answers[earlier.id]) : []
      const met = isOneOf.length ? isOneOf.some((wanted) => words.includes(wanted.trim().toLowerCase())) : words.length > 0
      if (!met) continue
    }
    shown.push(field)
  }
  return shown
}

export function normalizeForm(value: unknown): PublicForm {
  const raw = asRecord(value)
  return {
    name: asString(raw.name),
    collection: asString(raw.collection),
    titleName: asString(raw.titleName) || 'Title',
    intro: asString(raw.intro),
    thanks: asString(raw.thanks),
    renderToken: asString(raw.renderToken),
    honeypot: asString(raw.honeypot),
    fields: normalizeQuestions(raw.fields),
  }
}

/** The first thing the person still has to fill in, in words, or null. */
export function missing(form: Pick<PublicForm, 'titleName' | 'fields'>, answers: Answers): string | null {
  const title = answers.title
  if (typeof title !== 'string' || !title.trim()) return `Fill in ${form.titleName}.`
  for (const field of shownFields(form.fields, answers)) {
    if (!field.required || field.type === 'checkbox') continue
    const value = answers[field.id]
    const empty = value === undefined || (typeof value === 'string' && !value.trim()) || (Array.isArray(value) && value.length === 0)
    if (empty) return `Fill in ${field.name}.`
  }
  return null
}

/** The answers as the report URL takes them: by field ID, blanks left out. */
export function answerBody(
  form: Pick<PublicForm, 'fields'> & Partial<Pick<PublicForm, 'renderToken' | 'honeypot'>>,
  answers: Answers,
): Record<string, string | string[] | boolean | number> {
  const body: Record<string, string | string[] | boolean | number> = { title: String(answers.title ?? '').trim() }
  if (form.renderToken) body._aw_rendered = form.renderToken
  const trap = form.honeypot ? answers[form.honeypot] : undefined
  if (form.honeypot && typeof trap === 'string' && trap) body[form.honeypot] = trap
  for (const field of shownFields(form.fields, answers)) {
    const value = answers[field.id]
    if (value === undefined) continue
    if (typeof value === 'string') {
      if (!value.trim()) continue
      body[field.id] = field.type === 'number' && Number.isFinite(Number(value)) ? Number(value) : value.trim()
    } else if (Array.isArray(value)) {
      if (value.length) body[field.id] = value
    } else {
      body[field.id] = value
    }
  }
  return body
}

/** Reads the form; a closed or missing one says so. */
export async function fetchForm(token: string): Promise<PublicForm> {
  const response = await fetch(apiUrl(`/api/v1/forms/${encodeURIComponent(token)}`), { credentials: 'omit' })
  const body: unknown = await response.json().catch(() => null)
  if (!response.ok) throw new FormProblem(response.status, asString(asRecord(body).error) || 'This form didn’t load.')
  return normalizeForm(body)
}

/** Sends the answers; resolves with the thank-you note. */
export async function sendAnswers(token: string, form: PublicForm, answers: Answers): Promise<string> {
  const response = await fetch(apiUrl(`/api/v1/bug-reports/${encodeURIComponent(token)}`), {
    method: 'POST',
    credentials: 'omit',
    headers: { 'Content-Type': 'application/json' },
    body: JSON.stringify(answerBody(form, answers)),
  })
  const body = asRecord(await response.json().catch(() => null))
  if (!response.ok) throw new FormProblem(response.status, asString(body.error) || 'Your answers weren’t sent. Try again.')
  return asString(body.thanks)
}

export class FormProblem extends Error {
  constructor(
    readonly status: number,
    message: string,
  ) {
    super(message)
  }
}
