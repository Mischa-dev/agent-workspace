import type { LiveHandler, WebModule } from './define'
import { WEB_MODULES } from './registry'

// M1.5: the live-update events modules answer. The socket hands an event of
// a registered type to its module's handler, loaded the first time one
// arrives; the handler refreshes exactly what the event names.

/** Which module answers each event type (the first to register it). */
export function liveEventOwners(modules: readonly WebModule[] = WEB_MODULES): Map<string, WebModule> {
  const owners = new Map<string, WebModule>()
  for (const module of modules) {
    for (const registration of module.live ?? []) {
      for (const type of registration.events) if (!owners.has(type)) owners.set(type, module)
    }
  }
  return owners
}

const OWNERS = liveEventOwners()

/** Whether a module answers this event type. */
export function liveEventRegistered(type: string) {
  return OWNERS.has(type)
}

const loaded = new Map<string, Promise<LiveHandler>>()

/** The handler for this event type, loading once; null when no module answers it. */
export function liveHandlerFor(type: string, modules: readonly WebModule[] = WEB_MODULES): Promise<LiveHandler> | null {
  const owners = modules === WEB_MODULES ? OWNERS : liveEventOwners(modules)
  const module = owners.get(type)
  const registration = module?.live?.find((entry) => entry.events.includes(type))
  if (!module || !registration) return null
  const key = `${module.id}:${registration.events.join(',')}`
  let handler = loaded.get(key)
  if (!handler) {
    handler = registration.handler().then((found) => found.default)
    // A failed load is tried again with the next event.
    handler.catch(() => loaded.delete(key))
    loaded.set(key, handler)
  }
  return handler
}
