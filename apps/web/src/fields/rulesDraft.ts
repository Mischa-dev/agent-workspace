import type { RulesInput } from './api'
import type { FieldDefinition, FieldType } from './fieldModel'

/** A field's rules as the field dialog edits them (T1.19). Numbers and the default are kept as typed. */
export type RulesDraft = {
  required: boolean
  unique: boolean
  buildersOnly: boolean
  /** As typed: a value, an option's name, "me", "today" or "true"; '' for none. */
  default: string
  min: string
  max: string
  pattern: string
}

/** How a type's default is picked, or null when it has none. */
export type DefaultKind = 'typed' | 'number' | 'choice' | 'me' | 'today' | 'checked' | null

const TAKE_RULES: readonly FieldType[] = [
  'text',
  'long_text',
  'number',
  'date',
  'single_choice',
  'multiple_choice',
  'status',
  'person',
  'url',
  'email',
  'phone',
  'checkbox',
  'link',
  'relation',
]

/** Whether a field of this type takes rules: one whose value someone writes. */
export const takesRules = (type: FieldType) => TAKE_RULES.includes(type)
export const takesUnique = (type: FieldType) => ['text', 'email', 'url', 'phone', 'number'].includes(type)
export const takesRange = (type: FieldType) => type === 'number'
export const takesPattern = (type: FieldType) => type === 'text' || type === 'long_text'

export function defaultKind(type: FieldType): DefaultKind {
  switch (type) {
    case 'text':
    case 'long_text':
    case 'url':
    case 'email':
    case 'phone':
      return 'typed'
    case 'number':
      return 'number'
    case 'single_choice':
    case 'status':
      return 'choice'
    case 'person':
      return 'me'
    case 'date':
      return 'today'
    case 'checkbox':
      return 'checked'
    default:
      return null
  }
}

export function rulesDraft(field: FieldDefinition | null): RulesDraft {
  const rules = field?.rules
  const stored = rules?.default
  // A choice's default is stored by option ID: shown by its name.
  const option = typeof stored === 'string' ? field?.options.find((entry) => entry.id === stored) : undefined
  return {
    required: rules?.required ?? false,
    unique: rules?.unique ?? false,
    buildersOnly: rules?.buildersOnly ?? false,
    default: option ? option.name : stored === null || stored === undefined || stored === false ? '' : String(stored),
    min: rules?.min === null || rules?.min === undefined ? '' : String(rules.min),
    max: rules?.max === null || rules?.max === undefined ? '' : String(rules.max),
    pattern: rules?.pattern ?? '',
  }
}

/** The rules to save, or what to fix first. */
export function rulesInput(draft: RulesDraft, type: FieldType): { input: RulesInput } | { problem: string } {
  const input: RulesInput = {}
  if (draft.required) input.required = true
  if (draft.unique && takesUnique(type)) input.unique = true
  if (draft.buildersOnly) input.buildersOnly = true
  if (takesRange(type)) {
    for (const [key, typed] of [
      ['min', draft.min],
      ['max', draft.max],
    ] as const) {
      if (!typed.trim()) continue
      const number = Number(typed)
      if (!Number.isFinite(number)) return { problem: `The ${key === 'min' ? 'smallest' : 'largest'} value is a number.` }
      input[key] = number
    }
  }
  if (takesPattern(type) && draft.pattern.trim()) input.pattern = draft.pattern.trim()
  const kind = defaultKind(type)
  const typed = draft.default.trim()
  if (kind && typed) {
    if (kind === 'number') {
      const number = Number(typed)
      if (!Number.isFinite(number)) return { problem: 'The default is a number.' }
      input.default = number
    } else if (kind === 'checked') {
      input.default = true
    } else {
      input.default = typed
    }
  }
  return { input }
}
