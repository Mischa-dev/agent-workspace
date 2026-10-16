import { useId, useState, type FormEvent } from 'react'
import { useMutation, useQueryClient } from '@tanstack/react-query'

import { isApiError } from '../api/errors'
import { describeError } from '../app/mutations'
import { Banner, Button, SegmentedControl, SelectField, Switch, TextField } from '../ui'
import {
  formatSettingValue,
  resetSettingValue,
  setSettingValue,
  settingsKeys,
  sourceLabel,
  viewAtScope,
  type EffectiveSettings,
  type ResolvedSetting,
  type SettingDefinition,
  type SettingOption,
  type SettingsTarget,
} from './api'

type Write = { kind: 'set'; value: unknown } | { kind: 'reset' }

/**
 * Any registry setting at one scope: what it inherits and from where, a switch
 * to override it here, the control for its type, and Reset. Writes carry the
 * version they were based on; when someone else got there first it shows the
 * latest value and says so.
 */
export function SettingField({
  canEdit,
  definition,
  options,
  resolved,
  target,
}: {
  canEdit: boolean
  definition: SettingDefinition
  /** Names for an identifier's values, such as repositories for `run.repository`. */
  options?: SettingOption[]
  resolved: ResolvedSetting
  target: SettingsTarget
}) {
  const queryClient = useQueryClient()
  const id = useId()
  const view = viewAtScope(resolved, target.scope)
  const overridden = view.own !== null
  const [notice, setNotice] = useState<{ tone: 'warning' | 'danger'; text: string } | null>(null)
  const queryKey = settingsKeys.effective(target)

  const write = useMutation({
    mutationFn: (input: Write) => {
      const expectedVersion = view.own?.version ?? 0
      return input.kind === 'set'
        ? setSettingValue(target, definition.key, input.value, expectedVersion)
        : resetSettingValue(target, definition.key, expectedVersion)
    },
    onMutate: () => setNotice(null),
    onSuccess: ({ setting }) => {
      queryClient.setQueryData<EffectiveSettings>(queryKey, (current) =>
        current
          ? { ...current, settings: current.settings.map((candidate) => (candidate.key === setting.key ? setting : candidate)) }
          : current,
      )
    },
    onError: async (error) => {
      if (isApiError(error) && (error.code === 'version_conflict' || error.status === 409)) {
        await queryClient.invalidateQueries({ queryKey })
        setNotice({ tone: 'warning', text: 'Someone else changed this setting just now. The latest value is shown; make your change again if you still want it.' })
      } else {
        setNotice({ tone: 'danger', text: describeError(error).message })
      }
    },
  })

  const scopeName = target.scope === 'project' ? 'this Project' : 'the workspace'
  const inheritedText = formatSettingValue(view.inherited.value, definition.valueType, options)
  const busy = write.isPending
  const editable = canEdit && overridden && !busy

  return (
    <div className={overridden ? 'aw-setting is-overridden' : 'aw-setting'} id={`setting-${definition.key}`} tabIndex={-1}>
      <div className="aw-setting__head">
        <div className="aw-setting__text">
          <h3 className="aw-setting__label" id={`${id}-label`}>
            {definition.label}
          </h3>
          <p className="aw-setting__desc">{definition.description}</p>
        </div>
        {canEdit && (
          <div className="aw-setting__override">
            <Switch
              checked={overridden}
              disabled={busy}
              label={target.scope === 'project' ? 'Override' : 'Set here'}
              onChange={(checked) => write.mutate(checked ? { kind: 'set', value: view.inherited.value } : { kind: 'reset' })}
            />
          </div>
        )}
      </div>
      <p className="aw-setting__source">
        {overridden ? (
          <>
            Set for {scopeName}. Without it: {inheritedText} (from {sourceLabel(view.inherited.source)}).
          </>
        ) : view.inherited.source === 'default' ? (
          <>Built-in default: {inheritedText}</>
        ) : (
          <>
            Inherited from {sourceLabel(view.inherited.source)}: {inheritedText}
          </>
        )}
      </p>
      {overridden || !canEdit ? (
        <div className="aw-setting__control">
          {canEdit ? (
            <SettingControl
              definition={definition}
              disabled={!editable}
              labelledBy={`${id}-label`}
              onSave={(value) => write.mutate({ kind: 'set', value })}
              options={options}
              value={view.value}
            />
          ) : (
            <p className="aw-setting__value">{formatSettingValue(view.value, definition.valueType, options)}</p>
          )}
          {canEdit && (
            <Button disabled={busy} icon="undo" onClick={() => write.mutate({ kind: 'reset' })} size="sm" variant="ghost">
              Reset
            </Button>
          )}
        </div>
      ) : null}
      {notice && <Banner tone={notice.tone}>{notice.text}</Banner>}
    </div>
  )
}

function SettingControl({
  definition,
  disabled,
  labelledBy,
  onSave,
  options,
  value,
}: {
  definition: SettingDefinition
  disabled: boolean
  labelledBy: string
  onSave: (value: unknown) => void
  options?: SettingOption[]
  value: unknown
}) {
  const type = definition.valueType
  if (type.type === 'bool') {
    return (
      <Switch
        checked={value === true}
        disabled={disabled}
        label={value === true ? 'On' : 'Off'}
        onChange={(checked) => onSave(checked)}
      />
    )
  }
  if (type.type === 'choice') {
    const choices = type.options.map((option) => ({ value: option, label: formatSettingValue(option, type) }))
    if (choices.length <= 4 && !disabled) {
      return (
        <SegmentedControl
          label={definition.label}
          onChange={(next) => next !== value && onSave(next)}
          options={choices}
          value={typeof value === 'string' ? value : choices[0]?.value ?? ''}
        />
      )
    }
    return (
      <SelectField aria-labelledby={labelledBy} disabled={disabled} onChange={(event) => onSave(event.target.value)} value={String(value ?? '')}>
        {choices.map((choice) => (
          <option key={choice.value} value={choice.value}>
            {choice.label}
          </option>
        ))}
      </SelectField>
    )
  }
  if (options && type.type === 'identifier') {
    return (
      <SelectField
        aria-labelledby={labelledBy}
        disabled={disabled}
        onChange={(event) => onSave(event.target.value === '' ? null : event.target.value)}
        value={typeof value === 'string' ? value : ''}
      >
        {type.optional && <option value="">Not set</option>}
        {typeof value === 'string' && value && !options.some((option) => option.value === value) && <option value={value}>{value}</option>}
        {options.map((option) => (
          <option key={option.value} value={option.value}>
            {option.label}
          </option>
        ))}
      </SelectField>
    )
  }
  // A new saved value (here or elsewhere) starts a fresh draft.
  return <TextControl definition={definition} key={value === null || value === undefined ? '' : String(value)} disabled={disabled} labelledBy={labelledBy} onSave={onSave} value={value} />
}

/** Integers, text and identifiers: edit, then save with Enter or the button. */
function TextControl({
  definition,
  disabled,
  labelledBy,
  onSave,
  value,
}: {
  definition: SettingDefinition
  disabled: boolean
  labelledBy: string
  onSave: (value: unknown) => void
  value: unknown
}) {
  const type = definition.valueType
  const saved = value === null || value === undefined ? '' : String(value)
  const [draft, setDraft] = useState(saved)
  const [error, setError] = useState<string | null>(null)

  const submit = (event: FormEvent<HTMLFormElement>) => {
    event.preventDefault()
    const trimmed = draft.trim()
    if (type.type === 'integer') {
      const number = Number(trimmed)
      if (!trimmed || !Number.isInteger(number) || number < type.min || number > type.max) {
        setError(`Enter a whole number from ${type.min} to ${type.max}.`)
        return
      }
      onSave(number)
      return
    }
    const optional = type.type === 'text' || type.type === 'identifier' ? type.optional : false
    if (!trimmed) {
      if (!optional) {
        setError('Enter a value.')
        return
      }
      onSave(null)
      return
    }
    if (type.type === 'identifier' && !/^[A-Za-z0-9._:/-]{1,200}$/.test(trimmed)) {
      setError('Use letters, digits and . _ : / - only.')
      return
    }
    if (type.type === 'text' && trimmed.length > type.maxChars) {
      setError(`Keep it under ${type.maxChars} characters.`)
      return
    }
    onSave(trimmed)
  }

  return (
    <form className="aw-setting__form" onSubmit={submit}>
      <TextField
        aria-labelledby={labelledBy}
        disabled={disabled}
        error={error}
        inputMode={type.type === 'integer' ? 'numeric' : undefined}
        mono={type.type === 'identifier'}
        onChange={(event) => setDraft(event.target.value)}
        placeholder={type.type === 'integer' ? undefined : 'Not set'}
        spellCheck={false}
        value={draft}
      />
      {draft !== saved && (
        <Button disabled={disabled} size="sm" type="submit" variant="primary">
          Save
        </Button>
      )}
    </form>
  )
}
