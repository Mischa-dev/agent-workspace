import type { FieldDefinition } from '../fields/fieldModel'
import type { FormSettings } from '../settings/project/bugReports'

/** A form being made or edited: which fields it asks, and whether each must be filled in. */
export type FormDraft = {
  name: string
  intro: string
  thanks: string
  asked: Record<string, 'optional' | 'required' | undefined>
  /** Answers a day before the form pauses itself, as typed. */
  dailyLimit: string
  /** Who fills it in (T1.28): anyone with its link, or the space's members in the app. */
  audience?: FormAudience
  /** A question shown only when an earlier one's answer is one of these option names. */
  conditions?: Record<string, FormCondition | undefined>
  /** A heading a question starts, as typed. */
  sections?: Record<string, string | undefined>
  /** Values the form sets on every record, kept as they were (agents set them). */
  sets?: FormSettings['sets']
  /** Section descriptions, kept as they were. */
  sectionDescriptions?: Record<string, string>
}

export type FormAudience = 'public' | 'internal'
export type FormCondition = { on: string; values: string[] }

/** The fields a condition can read: a choice or a checkbox. */
export function canBeCondition(field: FieldDefinition): boolean {
  return field.type === 'single_choice' || field.type === 'multiple_choice' || field.type === 'status' || field.type === 'checkbox'
}

/** The values a condition on this field picks from: its options' names, Yes for a checkbox. */
export function conditionValues(field: FieldDefinition): string[] {
  return field.type === 'checkbox' ? ['Yes'] : field.options.filter((option) => !option.archived).map((option) => option.name)
}

/** Answers a form takes in a day unless its maker says otherwise; the server's default. */
export const DEFAULT_DAILY_LIMIT = 500
export const MAX_DAILY_LIMIT = 10_000

/** What's wrong with the typed daily limit, in words, or null. */
export function dailyLimitProblem(draft: FormDraft): string | null {
  const limit = Number(draft.dailyLimit.trim())
  return Number.isInteger(limit) && limit >= 1 && limit <= MAX_DAILY_LIMIT ? null : `Answers a day is a whole number from 1 to ${MAX_DAILY_LIMIT.toLocaleString()}.`
}

/** Fields a form can ask. A public one: not people (they're this workspace's) or links (they point inside the project). An internal one asks for people too. */
export function askableFields(fields: FieldDefinition[], audience: FormAudience = 'public'): FieldDefinition[] {
  const never = audience === 'internal' ? ['link', 'relation'] : ['person', 'people', 'link', 'relation']
  return fields.filter((field) => !field.archived && !never.includes(field.type))
}

/** Where editing starts: a form's saved settings, or a new one asking nothing beyond the title. */
export function formDraft(
  saved: { name: string; form?: FormSettings } | null,
  collectionName: string,
  fields: FieldDefinition[],
  audience: FormAudience = 'public',
): FormDraft {
  if (!saved) return { name: `${collectionName} form`.slice(0, 80), intro: '', thanks: '', asked: {}, dailyLimit: String(DEFAULT_DAILY_LIMIT), audience }
  const known = new Set(fields.map((field) => field.id))
  const asked: FormDraft['asked'] = {}
  const conditions: NonNullable<FormDraft['conditions']> = {}
  for (const entry of saved.form?.fields ?? []) {
    if (!known.has(entry.field)) continue
    asked[entry.field] = entry.required ? 'required' : 'optional'
    if (entry.showIf) conditions[entry.field] = { on: entry.showIf.field, values: entry.showIf.isOneOf ?? [] }
  }
  const sections: NonNullable<FormDraft['sections']> = {}
  const sectionDescriptions: Record<string, string> = {}
  for (const section of saved.form?.sections ?? []) {
    sections[section.before] = section.title
    if (section.description) sectionDescriptions[section.before] = section.description
  }
  return {
    name: saved.name,
    intro: saved.form?.intro ?? '',
    thanks: saved.form?.thanks ?? '',
    asked,
    dailyLimit: String(saved.form?.dailyLimit ?? DEFAULT_DAILY_LIMIT),
    audience,
    conditions,
    sections,
    sectionDescriptions,
    ...(saved.form?.sets?.length ? { sets: saved.form.sets } : {}),
  }
}

/** What the server stores: the asked fields in the collection's order. */
export function formFromDraft(draft: FormDraft, fields: FieldDefinition[]): FormSettings {
  const limit = Number(draft.dailyLimit.trim())
  const askedIds = fields.filter((field) => draft.asked[field.id]).map((field) => field.id)
  const sections = askedIds.flatMap((id) => {
    const title = draft.sections?.[id]?.trim()
    if (!title) return []
    const description = draft.sectionDescriptions?.[id]
    return [{ title, before: id, ...(description ? { description } : {}) }]
  })
  return {
    ...(dailyLimitProblem(draft) === null && limit !== DEFAULT_DAILY_LIMIT && draft.audience !== 'internal' ? { dailyLimit: limit } : {}),
    intro: draft.intro.trim(),
    thanks: draft.thanks.trim(),
    fields: fields.flatMap((field) => {
      const asked = draft.asked[field.id]
      if (!asked) return []
      const entry: FormSettings['fields'][number] = asked === 'required' && field.type !== 'checkbox' ? { field: field.id, required: true } : { field: field.id }
      // A condition reads a question asked before this one.
      const condition = draft.conditions?.[field.id]
      if (condition && askedIds.indexOf(condition.on) > -1 && askedIds.indexOf(condition.on) < askedIds.indexOf(field.id)) {
        entry.showIf = { field: condition.on, isOneOf: condition.values }
      }
      return [entry]
    }),
    ...(sections.length ? { sections } : {}),
    ...(draft.sets?.length ? { sets: draft.sets } : {}),
  }
}
