import { useWorkspaceLayout } from './api'
import { PresetCards } from './LayoutEditor'
import { presetChoices } from './presetChoices'
import { layoutShape, presetName } from './registry'

/** "What's this project for?" on New project: a preset, or the workspace's default. */
export default function PresetPicker({
  onChange,
  organizationId,
  value,
}: {
  onChange: (preset: string | null) => void
  organizationId: string
  value: string | null
}) {
  const workspace = useWorkspaceLayout(organizationId)
  const saved = workspace.data?.savedPresets ?? []
  const fallback = workspace.data?.layout ?? null
  const choices = presetChoices(saved, {
    name: 'Workspace default',
    description: fallback ? `Like other projects here: ${presetName(fallback.preset, saved)}.` : 'Like other projects here: everything shows.',
    shape: layoutShape(fallback, saved),
  })
  return (
    <fieldset className="aw-presetpicker">
      <legend className="aw-presetpicker__legend">What’s this project for?</legend>
      <p className="aw-presetpicker__hint">It decides what the project shows and what it calls things. You can change it later in the project’s settings.</p>
      <PresetCards choices={choices} label="What’s this project for?" onChange={onChange} value={value} />
    </fieldset>
  )
}
