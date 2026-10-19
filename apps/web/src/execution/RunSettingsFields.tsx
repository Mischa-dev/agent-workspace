import { SelectField } from '../ui'
import { pickableModels, type CatalogModel, type Harness, type RunSettings } from './api'
import { reasoningLabel } from './labels'

/** Keeps a harness and a model apart in one option value; identifiers never hold a space. */
const SEPARATOR = ' '

/**
 * Model, reasoning and speed pickers drawn from the harnesses' model lists. With
 * one harness it offers that harness's models. With several (nobody picked the
 * harness yet) the models are grouped by harness, and picking one picks its
 * harness too, since a model only runs on a harness that lists it. An empty
 * choice means "use the default"; a saved value the list no longer offers stays
 * shown until it's changed.
 */
export function RunSettingsFields({
  harnesses,
  idPrefix,
  onChange,
  value,
}: {
  harnesses: Harness[]
  idPrefix: string
  onChange: (value: RunSettings) => void
  value: RunSettings
}) {
  const single = harnesses.length === 1 ? harnesses[0] : null
  const choices = harnesses
    .map((harness) => ({ harness, models: pickableModels(harness) }))
    .filter((choice) => choice.models.length > 0)
  if (!choices.length) return null
  const models = single ? pickableModels(single) : []
  const fallback = models.find((candidate) => candidate.isDefault) ?? models[0]
  const picked = models.find((candidate) => candidate.id === value.model)
  // Reasoning and speed follow the model that will run: the one picked, else the default.
  const model = single ? (value.model ? picked : fallback) : undefined
  const stale = Boolean(value.model) && single !== null && !picked
  const pick = (next: string) => {
    if (!next) {
      onChange({ ...value, model: undefined, reasoning: undefined, speed: undefined })
      return
    }
    const [harness, modelId] = single ? [single.kind, next] : next.split(SEPARATOR)
    onChange({ ...value, harness, model: modelId, reasoning: undefined, speed: undefined })
  }

  return (
    <div className="aw-inline-form">
      <SelectField id={`${idPrefix}-model`} label="Model" onChange={(event) => pick(event.target.value)} value={single ? (value.model ?? '') : ''}>
        <option value="">{fallback ? `Default (${fallback.displayName})` : 'The harness’s default'}</option>
        {stale ? <option value={value.model}>{value.model} (not in the list)</option> : null}
        {single
          ? models.map((candidate) => (
              <option key={candidate.id} value={candidate.id}>
                {candidate.displayName}
              </option>
            ))
          : choices.map(({ harness, models: listed }) => (
              <optgroup key={harness.kind} label={harness.displayName}>
                {listed.map((candidate) => (
                  <option key={candidate.id} value={`${harness.kind}${SEPARATOR}${candidate.id}`}>
                    {candidate.displayName}
                  </option>
                ))}
              </optgroup>
            ))}
      </SelectField>
      {model && model.reasoningLevels.length > 0 ? (
        <SelectField
          id={`${idPrefix}-reasoning`}
          label="Reasoning"
          onChange={(event) => onChange({ ...value, reasoning: event.target.value || undefined })}
          value={value.reasoning ?? ''}
        >
          <option value="">Default{model.defaultReasoning ? ` (${reasoningLabel(model.defaultReasoning)})` : ''}</option>
          {offered(model.reasoningLevels, value.reasoning).map((level) => (
            <option key={level} value={level}>
              {model.reasoningLevels.includes(level) ? reasoningLabel(level) : `${reasoningLabel(level)} (not offered)`}
            </option>
          ))}
        </SelectField>
      ) : null}
      {model && model.speeds.length > 1 ? (
        <SelectField
          id={`${idPrefix}-speed`}
          label="Speed"
          onChange={(event) => onChange({ ...value, speed: event.target.value || undefined })}
          value={value.speed ?? ''}
        >
          <option value="">Default{speedName(model, model.defaultSpeed)}</option>
          {offered(
            model.speeds.map((speed) => speed.id),
            value.speed,
          ).map((speed) => (
            <option key={speed} value={speed}>
              {model.speeds.find((option) => option.id === speed)?.name ?? `${speed} (not offered)`}
            </option>
          ))}
        </SelectField>
      ) : null}
    </div>
  )
}

/** The model's choices, plus a saved one it doesn't offer so it stays visible. */
function offered(choices: string[], saved: string | undefined) {
  return saved && !choices.includes(saved) ? [...choices, saved] : choices
}

function speedName(model: CatalogModel, speed: string | null | undefined) {
  const name = model.speeds.find((option) => option.id === speed)?.name
  return name ? ` (${name})` : ''
}
