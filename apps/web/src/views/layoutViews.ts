import type { Experience } from '../experience/layout'
import type { PartId } from '../experience/parts'
import type { SavedView } from './api'

/** Whether the project's layout shows a saved view: Ready for agents goes with runs. */
export function viewShown(view: Pick<SavedView, 'builtin'>, experience: Experience): boolean {
  return view.builtin !== 'ready_for_agents' || experience.shows('area.runs')
}

/** A view's name in the layout's words: My tasks is "My to-dos" in Personal life. */
export function viewName(view: Pick<SavedView, 'builtin' | 'name'>, experience: Experience): string {
  return view.builtin === 'my_tasks' ? `My ${experience.word('task', 'many')}` : view.name
}

/** The list columns a layout can hide, by the part that hides them. */
export const COLUMN_PARTS: Partial<Record<string, PartId>> = {
  priority: 'task.priority',
  key: 'tech.task_keys',
  due: 'task.dates',
}
