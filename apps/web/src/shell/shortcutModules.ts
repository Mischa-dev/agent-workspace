import shortcutClaims from 'virtual:modules/slots/shortcuts'

import type { Experience } from '../experience/layout'
import { moduleSwitchable, ownerOf } from '../modules/ownership'

// AWD-75: shortcuts that act in the open project; the rest go to pages
// across the workspace, such as Inbox, Messages and My work.
const IN_PROJECT = /^(create-task|go-(tasks|board|pages|decisions))$/

/**
 * Whether a shortcut's module is on where it acts: in the open project
 * (`project`, null when none is open) for a project's own actions, at Home
 * for the rest; never while those settings load. The keys and the shortcut
 * sheet both ask this, so the sheet lists what the keys do.
 */
export function shortcutOn(action: string, project: Experience | null, home: Experience) {
  const module = ownerOf(shortcutClaims, action)
  if (!module || !moduleSwitchable(module)) return true
  const experience = project && IN_PROJECT.test(action) ? project : home
  return experience.modules !== 'loading' && experience.moduleOn(module)
}
