import { isApiError } from '../api/errors'
import { projectSettingsHref } from '../settings/paths'

export type PlanningTab = 'next' | 'suggestions' | 'graph' | 'plan-import'

/** Where each planning page lives. */
export const planningHref = (projectId: string, tab: PlanningTab) => `/projects/${encodeURIComponent(projectId)}/${tab}`

/** Settings link for a project's suggestions and summaries, part of its Runs and models settings. */
export const projectAiSettingsHref = (projectId: string) => `${projectSettingsHref(projectId, 'runs')}#suggestions`

/** What an AI call's error means, in words a person can act on. */
export function aiErrorMessage(error: unknown, doing = 'That') {
  if (isApiError(error)) {
    if (error.code === 'ai_not_configured' || error.code === 'ai_budget_reached') return error.message
    if (error.status === 403) return 'You can’t do that in this project.'
    if (error.status === 404) return 'That isn’t there any more, or you can’t see it.'
    if (error.message) return error.message
  }
  return `${doing} didn’t work. Check your connection and try again.`
}
