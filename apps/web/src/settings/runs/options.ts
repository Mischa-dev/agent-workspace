import { pickableModels, type Harness } from '../../execution/api'
import { reasoningLabel } from '../../execution/labels'
import { viewAtScope, type EffectiveSettings, type SettingOption, type SettingsTarget } from '../api'

/** What leaving a run setting unset means, shown in its place. */
export const RUN_UNSET_LABELS: Record<string, string> = {
  'run.harness': 'Any you’ve signed in to',
  'run.model': 'The harness’s default',
  'run.environment': 'This server',
  'run.repository': 'Ask each time',
}

/** The same for the settings behind suggestions and summaries. */
export const SUGGESTION_UNSET_LABELS: Record<string, string> = {
  'ai.provider': 'Not chosen yet',
  'ai.model': 'The provider’s default',
  'ai.deep_model': 'The provider’s default',
  'ai.base_url': 'The provider’s usual address',
}

/** A setup problem from the server says where to fix it, which is this page; drop that part here. */
export const here = (problem: string) => problem.replace(' in Runs and models settings', '')

/** The suggestion settings in the order people decide them: provider and models, the budget, then what runs on its own. */
export const SUGGESTION_KEYS = [
  'ai.provider',
  'ai.base_url',
  'ai.model',
  'ai.deep_model',
  'ai.monthly_budget_usd',
  'ai.background_detection',
  'ai.auto_apply_suggestions',
  'ai.auto_apply_min_confidence',
  'ai.',
]

/** The harness new runs use at `target`, if one is set there or above it. */
export function harnessAt(settings: EffectiveSettings | undefined, target: SettingsTarget) {
  const resolved = settings?.settings.find((setting) => setting.key === 'run.harness')
  const value = resolved ? viewAtScope(resolved, target.scope).value : null
  return typeof value === 'string' ? value : null
}

/**
 * Names for the run settings' values: the harnesses, the models people can pick
 * (the chosen harness's, else every harness's under its name) and plainer
 * words for some choices.
 */
export function runSettingOptions(harnesses: Harness[], harness: string | null): Record<string, SettingOption[]> {
  const chosen = harnesses.find((candidate) => candidate.kind === harness)
  return {
    'run.harness': harnesses.map((candidate) => ({ value: candidate.kind, label: candidate.displayName })),
    'run.model': (chosen ? [chosen] : harnesses).flatMap((candidate) =>
      pickableModels(candidate).map((model) => ({
        value: model.id,
        label: model.displayName,
        group: chosen ? undefined : candidate.displayName,
      })),
    ),
    'run.reasoning': [
      { value: 'default', label: 'Model’s default' },
      { value: 'xhigh', label: reasoningLabel('xhigh') },
    ],
    'run.speed': [{ value: 'default', label: 'Model’s default' }],
  }
}
