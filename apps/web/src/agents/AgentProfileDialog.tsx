import { useState, type FormEvent } from 'react'

import { useEnvironments } from '../launch/hooks'
import { Banner, Button, Dialog, SWATCHES, SelectField, TextArea, TextField, describeError, showToast } from '../ui'
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
  const [initial] = useState(() => draftFrom(profile ?? template))
  const [draft, setDraft] = useState(initial)
  const dirty = (Object.keys(draft) as (keyof Draft)[]).some((key) => draft[key] !== initial[key])
  const create = useCreateProfile(projectId)
  const update = useUpdateProfile(projectId)
  const environments = useEnvironments(projectId)
  const saving = create.isPending || update.isPending
  const error = create.error ?? update.error
  const harness = harnesses.find((choice) => choice.kind === draft.harness)
  const set = (patch: Partial<Draft>) => setDraft((current) => ({ ...current, ...patch }))
  const environmentChoices = (environments.data?.environments ?? []).filter((environment) => !draft.harness || environment.harness === draft.harness)

  const submit = (event: FormEvent<HTMLFormElement>) => {
    event.preventDefault()
    const input: ProfileInput = {
      name: draft.name.trim(),
      handle: draft.handle.trim(),
      avatar: draft.avatar.trim(),
      color: draft.color || undefined,
      description: draft.description.trim(),
      harness: draft.harness || null,
      runDefaults: { model: draft.model.trim() || null, reasoning: draft.reasoning.trim() || null, speed: draft.speed.trim() || null },
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
          <Button onClick={onClose}>Cancel</Button>
          <Button busy={saving} disabled={!draft.name.trim()} type="submit" variant="primary">
            {profile ? 'Save changes' : 'Add agent'}
          </Button>
        </>
      }
      dirty={dirty}
      onClose={onClose}
      onSubmit={submit}
      open
      title={profile ? `Edit ${profile.name}` : 'Add an agent'}
      width={600}
    >
      <div className="aw-profileform">
        {error ? (
          <Banner tone="danger" title={profile ? 'Your changes weren’t saved.' : 'The agent wasn’t added.'}>
            {describeError(error).message}
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
        <div className="aw-profileform__pair">
          <TextField hint="Up to 2 letters or an emoji." label="Avatar" maxLength={8} onChange={(event) => set({ avatar: event.target.value })} value={draft.avatar} />
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
        <SelectField label="Harness" onChange={(event) => set({ harness: event.target.value, environmentId: '' })} value={draft.harness}>
          <option value="">The project’s default</option>
          {harnesses.map((choice) => (
            <option key={choice.kind} value={choice.kind}>
              {choice.displayName}
            </option>
          ))}
        </SelectField>
        <div className="aw-profileform__triple">
          <TextField hint="Empty uses the harness default." label="Model" onChange={(event) => set({ model: event.target.value })} value={draft.model} />
          <TextField label="Reasoning" onChange={(event) => set({ reasoning: event.target.value })} value={draft.reasoning} />
          <TextField label="Speed" onChange={(event) => set({ speed: event.target.value })} value={draft.speed} />
        </div>
        <TextArea hint={instructionsHint(harness)} label="Instructions" onChange={(event) => set({ instructions: event.target.value })} rows={6} value={draft.instructions} />
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
          error={environments.isError ? 'Environments didn’t load. Close this and open it again to retry.' : undefined}
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
