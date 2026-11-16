import shortcutClaims from 'virtual:modules/slots/shortcuts'

import type { Experience } from '../experience/layout'
import type { WebModule } from '../modules/define'
import { moduleSwitchable, ownerOf } from '../modules/ownership'
import { moduleShortcuts, WEB_MODULES } from '../modules/registry'

// AWD-75: shortcuts that act in the open project; the rest go to pages
// across the workspace, such as Inbox, Messages and My work. A module's own
// keys say so in their registration (W1.13); the shell's are these.
const SHELL_IN_PROJECT = new Set(['create-task'])

/**
 * Whether a shortcut's module is on where it acts: in the open project
 * (`project`, null when none is open) for a project's own actions, at Home
 * for the rest; never while those settings load. The keys and the shortcut
 * sheet both ask this, so the sheet lists what the keys do. A registered
 * key follows the module that registered it.
 */
export function shortcutOn(
  action: string,
  project: Experience | null,
  home: Experience,
  modules: readonly WebModule[] = WEB_MODULES,
) {
  const registered = moduleShortcuts(modules).find((shortcut) => shortcut.action === action)
  const module = registered?.module ?? ownerOf(shortcutClaims, action)
  if (!module || !moduleSwitchable(module)) return true
  const inProject = registered ? Boolean(registered.inProject) : SHELL_IN_PROJECT.has(action)
  const experience = project && inProject ? project : home
  return experience.modules !== 'loading' && experience.moduleOn(module)
}
