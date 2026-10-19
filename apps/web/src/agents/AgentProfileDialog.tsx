import { useState, type FormEvent } from 'react'
import { useQuery } from '@tanstack/react-query'
import { Link } from 'react-router-dom'

import { queryKeys } from '../api/queryKeys'
import { listHarnesses, pickableModels, type Harness, type RunSettings } from '../execution/api'
import { RunSettingsFields } from '../execution/RunSettingsFields'
import { useEnvironments, useProjectSettings } from '../launch/hooks'
import { workspaceSettingsHref } from '../settings/paths'
import { Banner, Button, Dialog, SWATCHES, SelectField, TextArea, TextField, showToast } from '../ui'
import type { AgentProfile, ProfileHarness, ProfileInput, ProfileTemplate } from './api'
import { useCreateProfile, useUpdateProfile } from './hooks'
import { parseTools } from './presenceModel'

type Draft = {
  name: string
  handle: string
  avatar: string
  color: string
  description: string
  harness: string
  model: string
  reasoning: string
  speed: string
  instructions: string
  tools: string
  environmentId: string
}

function draftFrom(source: AgentProfile | ProfileTemplate | null): Draft {
  const profile = source && 'actorId' in source ? source : null
  return {
    name: source?.name ?? '',
    handle: source?.handle ?? '',
    avatar: source?.avatar ?? '',
    color: source?.color ?? '',
    description: source?.description ?? '',
    harness: profile?.harness ?? '',
    model: source?.runDefaults.model ?? '',
    reasoning: source?.runDefaults.reasoning ?? '',
    speed: source?.runDefaults.speed ?? '',
    instructions: source?.instructions ?? '',
    tools: profile?.allowedTools.join('\n') ?? '',
    environmentId: profile?.environmentId ?? '',
  }
}

/**
 * The model settings that still fit on `harness`: a model it doesn't list is
 * dropped, and so are a reasoning level or speed the model that would run
 * doesn't offer. Without a model list nothing can be checked, so it all goes.
 */
function fitTo(harness: Harness, draft: Pick<Draft, 'model' | 'reasoning' | 'speed'>) {
  const models = pickableModels(harness)
  const model = models.find((candidate) => candidate.id === draft.model)
  const runs = model ?? models.find((candidate) => candidate.isDefault) ?? models[0]
  return {
    model: model ? draft.model : '',
    reasoning: runs && (runs.reasoningLevels.length === 0 || runs.reasoningLevels.includes(draft.reasoning)) ? draft.reasoning : '',
    speed: runs?.speeds.some((speed) => speed.id === draft.speed) ? draft.speed : '',
  }
}

function instructionsHint(harness: ProfileHarness | undefined) {
  if (!harness) return 'Whether these go in as instructions or are added to the prompt depends on the project’s default harness.'
  return harness.takesInstructions
    ? `${harness.displayName} takes these as its instructions.`
    : `${harness.displayName} doesn’t take instructions, so these are added to the start of each prompt.`
}

/**
 * Creates an agent from a template (or blank) or edits one. Every field is
 * sent; the server fills what a template leaves out and says what it refuses.
 */
export function AgentProfileDialog({
  harnesses,
  onClose,
  profile,
  projectId,
  template,
}: {
  harnesses: ProfileHarness[]
  onClose: () => void
  profile: AgentProfile | null
  projectId: string
  template: ProfileTemplate | null
}) {
  const [draft, setDraft] = useState(() => draftFrom(profile ?? template))
  const create = useCreateProfile(projectId)
  const update = useUpdateProfile(projectId)
  const environments = useEnvironments(projectId)
  const catalogs = useQuery({ queryKey: queryKeys.harnesses(projectId), queryFn: () => listHarnesses(projectId) })
  const settings = useProjectSettings(projectId)
  const saving = create.isPending || update.isPending
  const error = create.error ?? update.error
  const harness = harnesses.find((choice) => choice.kind === draft.harness)
  const set = (patch: Partial<Draft>) => setDraft((current) => ({ ...current, ...patch }))
  const environmentChoices = (environments.data?.environments ?? []).filter((environment) => !draft.harness || environment.harness === draft.harness)
  // The harness this agent runs on: its own, else the project's default when one is set.
  const projectHarness = settings.data?.settings.find((setting) => setting.key === 'run.harness')?.value
  const defaultKind = typeof projectHarness === 'string' && harnesses.some((choice) => choice.kind === projectHarness) ? projectHarness : ''
  const runsOn = draft.harness || defaultKind
  const listed = (catalogs.data ?? []).filter((catalog) => harnesses.some((choice) => choice.kind === catalog.kind))
  const runsOnCatalog = listed.find((catalog) => catalog.kind === runsOn)
  const withoutModels = runsOnCatalog && pickableModels(runsOnCatalog).length === 0 ? runsOnCatalog : null
  const pickHarness = (kind: string) => {
    const next = listed.find((catalog) => catalog.kind === (kind || defaultKind))
    set({ harness: kind, environmentId: '', ...(next ? fitTo(next, draft) : {}) })
  }
  const pickSettings = (next: RunSettings) =>
    set({ harness: next.harness ?? draft.harness, model: next.model ?? '', reasoning: next.reasoning ?? '', speed: next.speed ?? '' })

  const submit = (event: FormEvent<HTMLFormElement>) => {
    event.preventDefault()
    // Settings the harness can't check would only stop the agent's runs.
    const runDefaults = runsOnCatalog ? fitTo(runsOnCatalog, draft) : draft
    const input: ProfileInput = {
      name: draft.name.trim(),
      handle: draft.handle.trim(),
      avatar: draft.avatar.trim(),
      color: draft.color || undefined,
      description: draft.description.trim(),
      harness: draft.harness || null,
      runDefaults: { model: runDefaults.model || null, reasoning: runDefaults.reasoning || null, speed: runDefaults.speed || null },
      instructions: draft.instructions,
      allowedTools: harness?.takesAllowedTools ? parseTools(draft.tools) : profile?.allowedTools ?? [],
      environmentId: draft.environmentId || null,
    }
    const done = (saved: AgentProfile) => {
      showToast({ tone: 'success', title: profile ? `${saved.name} saved` : `${saved.name} added` })
      onClose()
    }
    if (profile) update.mutate({ profileId: profile.id, input: { ...input, version: profile.version } }, { onSuccess: done })
    else create.mutate({ ...input, template: template?.id }, { onSuccess: done })
  }

  return (
    <Dialog
      description={profile ? undefined : template ? `Starts from the ${template.name} template. Change anything you like.` : 'Starts blank.'}
      footer={
        <>
          <Button onClick={onClose} variant="ghost">
            Cancel
          </Button>
          <Button busy={saving} disabled={!draft.name.trim()} type="submit" variant="primary">
            {profile ? 'Save' : 'Add agent'}
          </Button>
        </>
      }
      onClose={onClose}
      onSubmit={submit}
      open
      title={profile ? `Edit ${profile.name}` : 'Add an agent'}
      width={600}
    >
      <div className="aw-agentform">
        {error ? (
          <Banner tone="danger" title="Not saved.">
            {error.message}
          </Banner>
        ) : null}
        <TextField autoFocus label="Name" maxLength={80} onChange={(event) => set({ name: event.target.value })} required value={draft.name} />
        <TextField
          hint="Lowercase letters, digits, - and _. People mention the agent with it."
          label="Handle"
          maxLength={32}
          onChange={(event) => set({ handle: event.target.value })}
          placeholder="Made from the name"
          prefix="@"
          value={draft.handle}
        />
        <div className="aw-agentform__pair">
          <TextField hint="Up to 8 characters." label="Avatar" maxLength={8} onChange={(event) => set({ avatar: event.target.value })} value={draft.avatar} />
          <SelectField label="Color" onChange={(event) => set({ color: event.target.value })} value={draft.color}>
            <option value="">Pick for me</option>
            {draft.color && !(SWATCHES as readonly string[]).includes(draft.color) ? <option value={draft.color}>{draft.color}</option> : null}
            {SWATCHES.map((swatch) => (
              <option key={swatch} value={swatch}>
                {swatch[0].toUpperCase() + swatch.slice(1)}
              </option>
            ))}
          </SelectField>
        </div>
        <TextArea label="Description" maxLength={500} onChange={(event) => set({ description: event.target.value })} rows={2} value={draft.description} />
        <SelectField
          hint={
            withoutModels ? (
              // One piece, so the note's layout doesn't space the link apart.
              <span>
                {withoutModels.displayName} has nothing in its <Link to={`${workspaceSettingsHref('runs')}#models`}>model list</Link> yet, so
                this agent uses its defaults.
              </span>
            ) : undefined
          }
          label="Harness"
          onChange={(event) => pickHarness(event.target.value)}
          value={draft.harness}
        >
          <option value="">
            {defaultKind ? `The project’s default (${harnesses.find((choice) => choice.kind === defaultKind)?.displayName})` : 'The project’s default'}
          </option>
          {harnesses.map((choice) => (
            <option key={choice.kind} value={choice.kind}>
              {choice.displayName}
            </option>
          ))}
        </SelectField>
        <RunSettingsFields
          harnesses={runsOn ? listed.filter((catalog) => catalog.kind === runsOn) : listed}
          idPrefix="agent-profile"
          onChange={pickSettings}
          value={{ harness: draft.harness || undefined, model: draft.model || undefined, reasoning: draft.reasoning || undefined, speed: draft.speed || undefined }}
        />
        <TextArea hint={instructionsHint(harness ?? harnesses.find((choice) => choice.kind === defaultKind))} label="Instructions" onChange={(event) => set({ instructions: event.target.value })} rows={6} value={draft.instructions} />
        {harness?.takesAllowedTools ? (
          <TextArea
            hint="One per line. Empty lets the harness use its own defaults."
            label="Allowed tools"
            onChange={(event) => set({ tools: event.target.value })}
            rows={3}
            value={draft.tools}
          />
        ) : null}
        <SelectField
          error={environments.isError ? 'Environments didn’t load.' : undefined}
          hint={environmentChoices.length === 0 && environments.data ? 'No environment uses this harness yet.' : undefined}
          label="Environment"
          onChange={(event) => set({ environmentId: event.target.value })}
          value={draft.environmentId}
        >
          <option value="">The project’s default</option>
          {environmentChoices.map((environment) => (
            <option key={environment.id} value={environment.id}>
              {environment.name}
            </option>
          ))}
        </SelectField>
      </div>
    </Dialog>
  )
}
