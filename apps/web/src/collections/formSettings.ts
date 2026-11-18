import type { FieldDefinition } from '../fields/fieldModel'
import type { BugReportLink, FormSettings } from '../settings/project/bugReports'

/** A form being made or edited: which fields it asks, and whether each must be filled in. */
export type FormDraft = {
  name: string
  intro: string
  thanks: string
  asked: Record<string, 'optional' | 'required' | undefined>
  /** Answers a day before the form pauses itself, as typed. */
  dailyLimit: string
}

/** Answers a form takes in a day unless its maker says otherwise; the server's default. */
export const DEFAULT_DAILY_LIMIT = 500
export const MAX_DAILY_LIMIT = 10_000

/** What's wrong with the typed daily limit, in words, or null. */
export function dailyLimitProblem(draft: FormDraft): string | null {
  const limit = Number(draft.dailyLimit.trim())
  return Number.isInteger(limit) && limit >= 1 && limit <= MAX_DAILY_LIMIT ? null : `Answers a day is a whole number from 1 to ${MAX_DAILY_LIMIT.toLocaleString()}.`
}

/** Fields a public form can ask: not people (they're this workspace's) or links (they point inside the project). */
export function askableFields(fields: FieldDefinition[]): FieldDefinition[] {
  return fields.filter((field) => !field.archived && field.type !== 'person' && field.type !== 'link')
}

/** Where editing starts: a form's saved settings, or a new one asking nothing beyond the title. */
export function formDraft(link: BugReportLink | null, collectionName: string, fields: FieldDefinition[]): FormDraft {
  if (!link) return { name: `${collectionName} form`.slice(0, 80), intro: '', thanks: '', asked: {}, dailyLimit: String(DEFAULT_DAILY_LIMIT) }
  const known = new Set(fields.map((field) => field.id))
  const asked: FormDraft['asked'] = {}
  for (const entry of link.form?.fields ?? []) {
    if (known.has(entry.field)) asked[entry.field] = entry.required ? 'required' : 'optional'
  }
  return { name: link.name, intro: link.form?.intro ?? '', thanks: link.form?.thanks ?? '', asked, dailyLimit: String(link.form?.dailyLimit ?? DEFAULT_DAILY_LIMIT) }
}

/** What the server stores: the asked fields in the collection's order. */
export function formFromDraft(draft: FormDraft, fields: FieldDefinition[]): FormSettings {
  const limit = Number(draft.dailyLimit.trim())
  return {
    ...(dailyLimitProblem(draft) === null && limit !== DEFAULT_DAILY_LIMIT ? { dailyLimit: limit } : {}),
    intro: draft.intro.trim(),
    thanks: draft.thanks.trim(),
    fields: fields.flatMap((field) => {
      const asked = draft.asked[field.id]
      if (!asked) return []
      return [asked === 'required' && field.type !== 'checkbox' ? { field: field.id, required: true } : { field: field.id }]
    }),
  }
}
