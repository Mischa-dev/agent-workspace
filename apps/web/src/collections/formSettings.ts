import type { FieldDefinition } from '../fields/fieldModel'
import type { BugReportLink, FormSettings } from '../settings/project/bugReports'

/** A form being made or edited: which fields it asks, and whether each must be filled in. */
export type FormDraft = {
  name: string
  intro: string
  thanks: string
  asked: Record<string, 'optional' | 'required' | undefined>
}

/** Fields a public form can ask: not people (they're this workspace's) or links (they point inside the project). */
export function askableFields(fields: FieldDefinition[]): FieldDefinition[] {
  return fields.filter((field) => !field.archived && field.type !== 'person' && field.type !== 'link')
}

/** Where editing starts: a form's saved settings, or a new one asking nothing beyond the title. */
export function formDraft(link: BugReportLink | null, collectionName: string, fields: FieldDefinition[]): FormDraft {
  if (!link) return { name: `${collectionName} form`.slice(0, 80), intro: '', thanks: '', asked: {} }
  const known = new Set(fields.map((field) => field.id))
  const asked: FormDraft['asked'] = {}
  for (const entry of link.form?.fields ?? []) {
    if (known.has(entry.field)) asked[entry.field] = entry.required ? 'required' : 'optional'
  }
  return { name: link.name, intro: link.form?.intro ?? '', thanks: link.form?.thanks ?? '', asked }
}

/** What the server stores: the asked fields in the collection's order. */
export function formFromDraft(draft: FormDraft, fields: FieldDefinition[]): FormSettings {
  return {
    intro: draft.intro.trim(),
    thanks: draft.thanks.trim(),
    fields: fields.flatMap((field) => {
      const asked = draft.asked[field.id]
      if (!asked) return []
      return [asked === 'required' && field.type !== 'checkbox' ? { field: field.id, required: true } : { field: field.id }]
    }),
  }
}
