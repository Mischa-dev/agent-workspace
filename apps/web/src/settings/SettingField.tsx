import { useId, useState, type FormEvent, type ReactNode } from 'react'
import { useMutation, useQueryClient } from '@tanstack/react-query'
import { Link } from 'react-router-dom'

import { isApiError } from '../api/errors'
import { describeError } from '../app/mutations'
import { Banner, Button, SegmentedControl, SelectField, Switch, TextField } from '../ui'
import {
  formatSettingValue,
  resetSettingValue,
  scopeName,
  setSettingValue,
  settingsKeys,
  viewAtScope,
  type EffectiveSettings,
  type ResolvedSetting,
  type SettingDefinition,
  type SettingOption,
  type SettingScope,
  type SettingsTarget,
} from './api'

type Write = { kind: 'set'; value: unknown } | { kind: 'reset' }

/** A built-in default that only means "the default" goes without saying so. */
function saysDefault(value: unknown) {
  return value === null || value === undefined || value === '' || value === 'default'
}

/**
 * Any registry setting at one scope: the value that applies and where it's set,
 * a switch to set it here, the control for its type, and Reset. Writes carry the
 * version they were based on; when someone else got there first it shows the
 * latest value and says so.
 */
export function SettingField({
  canEdit,
  definition,
  onSaved,
  options,
  resolved,
  target,
  unsetLabel,
  workspaceHref,
}: {
  canEdit: boolean
  definition: SettingDefinition
  /** Called after a change is saved, for pages that show the same value elsewhere. */
  onSaved?: () => void
  /** Names for the setting's values, such as repositories for `run.repository`. */
  options?: SettingOption[]
  resolved: ResolvedSetting
  target: SettingsTarget
  /** What leaving it unset means, such as "The harness’s default". */
  unsetLabel?: string
  /** The workspace settings page with this setting, linked from "Set in: Workspace" on a project. */
  workspaceHref?: string
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
      onSaved?.()
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

  const busy = write.isPending
  const editable = canEdit && overridden && !busy
  const formatted = (value: unknown) => formatSettingValue(value, definition.valueType, options, unsetLabel)
  const where = (source: SettingScope | 'default'): ReactNode =>
    source === 'workspace' && target.scope !== 'workspace' && workspaceHref ? (
      <Link to={`${workspaceHref}#setting-${definition.key}`}>{scopeName(source)}</Link>
    ) : (
      scopeName(source)
    )
  // What applies here, and where it's set.
  const applied = overridden ? { source: target.scope, value: view.value } : view.inherited

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
        {canEdit && overridden ? (
          <>
            Set in: {where(target.scope)}. Without it: {formatted(view.inherited.value)}
            {view.inherited.source === 'default' ? null : <>, from {where(view.inherited.source)}</>}.
          </>
        ) : (
          <>
            <span className="aw-setting__current">{formatted(applied.value)}</span>
            {applied.source !== 'default' ? (
              <> · Set in: {where(applied.source)}</>
            ) : saysDefault(applied.value) ? null : (
              <> · {scopeName('default')}</>
            )}
          </>
        )}
      </p>
      {canEdit && overridden ? (
        <div className="aw-setting__control">
          <SettingControl
            definition={definition}
            disabled={!editable}
            labelledBy={`${id}-label`}
            onSave={(value) => write.mutate({ kind: 'set', value })}
            options={options}
            unsetLabel={unsetLabel}
            value={view.value}
          />
          <Button disabled={busy} icon="undo" onClick={() => write.mutate({ kind: 'reset' })} size="sm" variant="ghost">
            Reset
          </Button>
        </div>
      ) : null}
      {notice && <Banner tone={notice.tone}>{notice.text}</Banner>}
    </div>
  )
}

/** Options in their groups, in the order the groups first appear. */
function grouped(options: SettingOption[]) {
  const groups: { label: string | undefined; options: SettingOption[] }[] = []
  for (const option of options) {
    const group = groups.find((candidate) => candidate.label === option.group)
    if (group) group.options.push(option)
    else groups.push({ label: option.group, options: [option] })
  }
  return groups
}

function SettingControl({
  definition,
  disabled,
  labelledBy,
  onSave,
  options,
  unsetLabel,
  value,
}: {
  definition: SettingDefinition
  disabled: boolean
  labelledBy: string
  onSave: (value: unknown) => void
  options?: SettingOption[]
  unsetLabel?: string
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
    const choices = type.options.map((option) => ({ value: option, label: formatSettingValue(option, type, options) }))
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
        {type.optional && <option value="">{unsetLabel ?? 'Not set'}</option>}
        {typeof value === 'string' && value && !options.some((option) => option.value === value) && <option value={value}>{value}</option>}
        {grouped(options).map((group) =>
          group.label ? (
            <optgroup key={group.label} label={group.label}>
              {group.options.map((option) => (
                <option key={option.value} value={option.value}>
                  {option.label}
                </option>
              ))}
            </optgroup>
          ) : (
            group.options.map((option) => (
              <option key={option.value} value={option.value}>
                {option.label}
              </option>
            ))
          ),
        )}
      </SelectField>
    )
  }
  // A new saved value (here or elsewhere) starts a fresh draft.
  return (
    <TextControl
      definition={definition}
      disabled={disabled}
      key={value === null || value === undefined ? '' : String(value)}
      labelledBy={labelledBy}
      onSave={onSave}
      placeholder={unsetLabel}
      value={value}
    />
  )
}

/** Integers, text and identifiers: edit, then save with Enter or the button. */
function TextControl({
  definition,
  disabled,
  labelledBy,
  onSave,
  placeholder = 'Not set',
  value,
}: {
  definition: SettingDefinition
  disabled: boolean
  labelledBy: string
  onSave: (value: unknown) => void
  placeholder?: string
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
        placeholder={type.type === 'integer' ? undefined : placeholder}
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
