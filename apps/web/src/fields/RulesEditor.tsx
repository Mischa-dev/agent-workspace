import { Checkbox, SelectField, TextField } from '../ui'
import type { FieldType } from './fieldModel'
import { defaultKind, takesPattern, takesRange, takesUnique, type RulesDraft } from './rulesDraft'

/**
 * A collection field's rules (T1.19): whether every record needs a value,
 * whether two may share one, what a new record starts with, a number's
 * range, a text pattern, and who changes it. Record writes from anywhere
 * (the table, forms, agents, automations) meet them.
 */
export function RulesEditor({
  draft,
  onChange,
  options,
  type,
}: {
  draft: RulesDraft
  onChange: (draft: RulesDraft) => void
  /** A choice field's option names, for its default. */
  options: readonly string[]
  type: FieldType
}) {
  const kind = defaultKind(type)
  const set = (part: Partial<RulesDraft>) => onChange({ ...draft, ...part })
  return (
    <fieldset className="aw-fieldset">
      <legend className="aw-field__label">Rules</legend>
      <Checkbox checked={draft.required} label="Every record needs a value" onChange={(required) => set({ required })} />
      {takesUnique(type) ? <Checkbox checked={draft.unique} label="No two records share a value" onChange={(unique) => set({ unique })} /> : null}
      <Checkbox
        checked={draft.buildersOnly}
        label="Only people who can change the fields can change it"
        onChange={(buildersOnly) => set({ buildersOnly })}
      />
      {kind === 'me' ? (
        <Checkbox checked={draft.default === 'me'} label="Starts as whoever adds the record" onChange={(on) => set({ default: on ? 'me' : '' })} />
      ) : null}
      {kind === 'today' ? (
        <Checkbox checked={draft.default === 'today'} label="Starts as the day it’s added" onChange={(on) => set({ default: on ? 'today' : '' })} />
      ) : null}
      {kind === 'checked' ? <Checkbox checked={draft.default === 'true'} label="Starts checked" onChange={(on) => set({ default: on ? 'true' : '' })} /> : null}
      {kind === 'choice' ? (
        <SelectField label="Starts as" onChange={(event) => set({ default: event.target.value })} value={draft.default}>
          <option value="">Nothing</option>
          {options.map((name) => (
            <option key={name} value={name}>
              {name}
            </option>
          ))}
        </SelectField>
      ) : null}
      {(kind === 'typed' || kind === 'number') && !draft.unique ? (
        <TextField
          hint="Optional. What a new record starts with."
          inputMode={kind === 'number' ? 'decimal' : undefined}
          label="Starts as"
          maxLength={200}
          onChange={(event) => set({ default: event.target.value })}
          value={draft.default}
        />
      ) : null}
      {takesRange(type) ? (
        <div className="aw-estimate__row">
          <TextField compact inputMode="decimal" label="At least" onChange={(event) => set({ min: event.target.value })} value={draft.min} />
          <TextField compact inputMode="decimal" label="At most" onChange={(event) => set({ max: event.target.value })} value={draft.max} />
        </div>
      ) : null}
      {takesPattern(type) ? (
        <TextField
          hint="Optional. A regular expression the whole value matches, such as [A-Z]{3}-[0-9]+."
          label="Pattern"
          maxLength={200}
          onChange={(event) => set({ pattern: event.target.value })}
          value={draft.pattern}
        />
      ) : null}
    </fieldset>
  )
}
