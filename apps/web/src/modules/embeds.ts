import { lazy, type ComponentType, type LazyExoticComponent } from 'react'

import { legacyPathOf } from '../app/href'
import type { PartId } from '../experience/parts'
import type { EmbedParams, EmbedPickerProps, EmbedRegistration, EmbedViewProps, WebModule } from './define'
import { WEB_MODULES } from './registry'

// M1.5: what modules draw inside pages. A page's embed block keeps a path;
// the first registration whose `match` reads it draws it. Views and pickers
// load the first time they're used, once each.

export type FoundEmbed = { module: string; registration: EmbedRegistration; params: EmbedParams }

/** Every embed registration, with its module, in manifest order. */
export function moduleEmbeds(modules: readonly WebModule[] = WEB_MODULES): (EmbedRegistration & { module: string })[] {
  return modules.flatMap((module) => (module.embeds ?? []).map((embed) => ({ ...embed, module: module.id })))
}

/** The registration that reads this path (the app's own address), and what it read; null for any other. */
export function registeredEmbed(href: string, modules: readonly WebModule[] = WEB_MODULES): FoundEmbed | null {
  let url: URL
  try {
    url = new URL(href, 'https://app.invalid')
  } catch {
    return null
  }
  if (url.origin !== 'https://app.invalid') return null
  // Registrations read the old shape; a /w/<ws>/s/<space>/... address reads as it.
  const path = legacyPathOf(url.pathname)
  for (const module of modules) {
    for (const registration of module.embeds ?? []) {
      let params: EmbedParams | null
      try {
        params = registration.match(path, url.searchParams)
      } catch {
        // A path it can't decode isn't one of its.
        params = null
      }
      if (params) return { module: module.id, registration, params }
    }
  }
  return null
}

const views = new Map<string, LazyExoticComponent<ComponentType<EmbedViewProps>>>()
const pickers = new Map<string, LazyExoticComponent<ComponentType<EmbedPickerProps>>>()

/** The registration's view, loaded when first drawn (one lazy component per registration). */
export function embedView(module: string, registration: EmbedRegistration) {
  const key = `${module}:${registration.id}`
  let view = views.get(key)
  if (!view) {
    view = lazy(registration.view)
    views.set(key, view)
  }
  return view
}

/** The registration's picker, loaded when first opened. */
export function embedPicker(module: string, registration: EmbedRegistration) {
  const key = `${module}:${registration.id}`
  let picker = pickers.get(key)
  if (!picker && registration.picker) {
    picker = lazy(registration.picker.view)
    pickers.set(key, picker)
  }
  return picker ?? null
}

/** The pickers the page editor's toolbar offers here: their module is on and the layout shows their part. */
export function embedPickers(
  experience: { shows: (part: PartId) => boolean; moduleOn: (module: string) => boolean },
  modules: readonly WebModule[] = WEB_MODULES,
): (EmbedRegistration & { module: string; picker: NonNullable<EmbedRegistration['picker']> })[] {
  return moduleEmbeds(modules)
    .filter((embed): embed is EmbedRegistration & { module: string; picker: NonNullable<EmbedRegistration['picker']> } => Boolean(embed.picker))
    .filter((embed) => experience.moduleOn(embed.module))
    .filter((embed) => !embed.picker.part || experience.shows(embed.picker.part))
}
