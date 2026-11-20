import type { Experience } from '../experience/layout'
import type { PhoneTab, WebModule } from '../modules/define'
import { WEB_MODULES } from '../modules/registry'

// Apart from the registry so only the phone bar's own chunk carries it.

/** The phone's bottom bar (W1.29): each module that's on brings at most one destination. */
export function phoneTabs(
  experience: Pick<Experience, 'shows' | 'moduleOn'>,
  modules: readonly WebModule[] = WEB_MODULES,
): (PhoneTab & { module: string })[] {
  return modules
    .filter((module) => module.phone && experience.moduleOn(module.id))
    .flatMap((module) => (module.phone && (!module.phone.part || experience.shows(module.phone.part)) ? [{ ...module.phone, module: module.id }] : []))
    .sort((left, right) => left.rank - right.rank)
}
