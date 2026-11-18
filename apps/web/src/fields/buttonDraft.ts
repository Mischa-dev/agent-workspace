import catalog from '../api/generated/catalog.json'
import type { ButtonInput } from './api'
import { buttonArgumentsShown, type FieldDefinition } from './fieldModel'

/** One step as it's being written: an operation and its arguments as JSON text. */
export type StepDraft = { key: string; operation: string; arguments: string }
export type ButtonDraft = { label: string; steps: StepDraft[] }

export const MAX_STEPS = 3
const NEW_ARGUMENTS = '{\n  "spaceId": "{{space.id}}"\n}'

/** The operations a button can run: every change the app has, but pressing buttons. */
export const OPERATIONS = (catalog as Array<{ name: string; title: string; kind: string }>)
  .filter((entry) => ['create', 'change', 'suggest'].includes(entry.kind) && entry.name !== 'button_press')
  .sort((a, b) => a.title.localeCompare(b.title))

export const newStep = (): StepDraft => ({ key: crypto.randomUUID(), operation: OPERATIONS[0]?.name ?? '', arguments: NEW_ARGUMENTS })

/** A button field's settings as the dialog edits them: fields by today's names. */
export function buttonDraft(field: FieldDefinition | null, fields: readonly FieldDefinition[]): ButtonDraft {
  const button = field?.button
  if (!button) return { label: '', steps: [newStep()] }
  return {
    label: button.label,
    steps: button.steps.map((step) => ({
      key: crypto.randomUUID(),
      operation: step.operation,
      arguments: JSON.stringify(buttonArgumentsShown(step.arguments, fields), null, 2),
    })),
  }
}

/** The draft as the server takes it, or what to fix. */
export function buttonInput(draft: ButtonDraft): { input: ButtonInput } | { problem: string } {
  const steps: ButtonInput['steps'] = []
  for (const [index, step] of draft.steps.entries()) {
    let parsed: unknown
    try {
      parsed = step.arguments.trim() ? JSON.parse(step.arguments) : {}
    } catch {
      return { problem: `Step ${index + 1}’s arguments aren’t JSON. Write them as an object, such as {"title": "{{Name}}"}.` }
    }
    if (!parsed || typeof parsed !== 'object' || Array.isArray(parsed)) return { problem: `Step ${index + 1}’s arguments are an object, in braces.` }
    steps.push({ operation: step.operation, arguments: parsed as Record<string, unknown> })
  }
  return { input: { ...(draft.label.trim() ? { label: draft.label.trim() } : {}), steps } }
}
