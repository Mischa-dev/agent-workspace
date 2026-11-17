import { Button, SelectField, TextArea, TextField } from '../ui'
import { MAX_STEPS, OPERATIONS, newStep, type ButtonDraft, type StepDraft } from './buttonDraft'
import type { FormulaContext } from './FormulaEditor'

/**
 * A button field's button (T1.21): what it says and the operations pressing
 * it runs, in order, as whoever presses it. Arguments are JSON whose text can
 * name the record's fields in double braces.
 */
export function ButtonEditor({ context, draft, onChange }: { context: FormulaContext; draft: ButtonDraft; onChange: (draft: ButtonDraft) => void }) {
  const setStep = (key: string, change: Partial<StepDraft>) =>
    onChange({ ...draft, steps: draft.steps.map((step) => (step.key === key ? { ...step, ...change } : step)) })
  const names = [context.titleName, ...context.fields.filter((field) => !field.archived && field.type !== 'button').map((field) => field.name)]
  return (
    <div className="aw-buttonfield">
      <TextField
        hint="What the button says. The field’s name when left empty."
        label="Label"
        maxLength={40}
        onChange={(event) => onChange({ ...draft, label: event.target.value })}
        value={draft.label}
      />
      {draft.steps.map((step, index) => (
        <fieldset className="aw-buttonfield__step" key={step.key}>
          <legend>Step {index + 1}</legend>
          <SelectField label="Runs" onChange={(event) => setStep(step.key, { operation: event.target.value })} value={step.operation}>
            {OPERATIONS.some((entry) => entry.name === step.operation) ? null : <option value={step.operation}>{step.operation}</option>}
            {OPERATIONS.map((entry) => (
              <option key={entry.name} value={entry.name}>
                {entry.title} ({entry.name})
              </option>
            ))}
          </SelectField>
          <TextArea
            className="aw-formula__source"
            hint={`Its input as JSON. Fill in the record with {{${names.slice(0, 2).join('}}, {{')}}}, {{record.number}}, {{space.id}}, {{me}} or {{today}}.`}
            label="Arguments"
            onChange={(event) => setStep(step.key, { arguments: event.target.value })}
            rows={4}
            spellCheck={false}
            value={step.arguments}
          />
          {draft.steps.length > 1 ? (
            <Button onClick={() => onChange({ ...draft, steps: draft.steps.filter((other) => other.key !== step.key) })} size="sm" variant="ghost">
              Remove step {index + 1}
            </Button>
          ) : null}
        </fieldset>
      ))}
      {draft.steps.length < MAX_STEPS ? (
        <Button icon="plus" onClick={() => onChange({ ...draft, steps: [...draft.steps, newStep()] })} size="sm" variant="ghost">
          Add a step
        </Button>
      ) : null}
      <p className="aw-pagesection__desc">Steps run in order as whoever presses it, and stop at the first one that can’t go through.</p>
    </div>
  )
}
