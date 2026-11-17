// Applying a custom theme (a ThemeDocument, theme/contract.ts) to <html>: the
// built-in theme it starts from, its tokens as custom properties, and its CSS
// in one <style>. The document is cached in this browser so
// public/theme-boot.js paints it before the app loads; keep the two in step.
// The shell carries only this; the contract and its checks load with the
// Appearance page.

import type { ThemeDocument } from '../api/generated/operations'
import { DERIVED_SLOTS, isThemePreset, type ThemePreset } from './slots'

export const THEME_DOCUMENT_STORAGE_KEY = 'agent-workspace:theme-document:v1'
export const THEME_STYLE_ID = 'aw-theme-css'

/** A custom theme as this browser keeps it. */
export type CachedTheme = { id: string; workspaceId: string; version: number; name: string; document: ThemeDocument }
export type ModeName = 'light' | 'dark'

const TOKEN_NAME = /^[a-z0-9-]+$/
// The server checked the document; this is the last guard before CSS, for a cache someone edited by hand.
// eslint-disable-next-line no-control-regex
const UNSAFE_VALUE = /[;{}<>\\@!\u0000-\u001f]|\/\*|url\(/i
const UNSAFE_CSS = /[\\<]|url\(|@import|@font-face|@namespace|image-set\(|image\(|src\(/i

function record(value: unknown): Record<string, unknown> | null {
  return value && typeof value === 'object' && !Array.isArray(value) ? (value as Record<string, unknown>) : null
}

/** The theme cached in this browser, or null. */
export function readCachedTheme(storage: Pick<Storage, 'getItem'> = window.localStorage): CachedTheme | null {
  try {
    const found = record(JSON.parse(storage.getItem(THEME_DOCUMENT_STORAGE_KEY) ?? 'null'))
    const document = record(found?.document)
    if (!found || !document || typeof found.id !== 'string' || typeof found.workspaceId !== 'string') return null
    if (!record(document.light) || !record(document.dark)) return null
    return {
      id: found.id,
      workspaceId: found.workspaceId,
      version: typeof found.version === 'number' ? found.version : 0,
      name: typeof found.name === 'string' ? found.name : '',
      document: document as ThemeDocument,
    }
  } catch {
    return null
  }
}

export function writeCachedTheme(theme: CachedTheme | null, storage: Pick<Storage, 'setItem' | 'removeItem'> = window.localStorage) {
  try {
    if (theme) storage.setItem(THEME_DOCUMENT_STORAGE_KEY, JSON.stringify(theme))
    else storage.removeItem(THEME_DOCUMENT_STORAGE_KEY)
  } catch {
    // Best effort: the theme still applies for this visit.
  }
}

/** The built-in theme a document's mode starts from. */
export function documentBase(document: ThemeDocument, mode: ModeName): ThemePreset {
  const base = document[mode]?.base
  if (isThemePreset(base)) return base
  return mode === 'dark' ? 'quiet-dark' : 'quiet'
}

/** The custom properties a document's mode sets: its tokens, plus derived colours whose base changed. */
export function documentProperties(document: ThemeDocument, mode: ModeName): Record<string, string> {
  const tokens: Record<string, string> = {}
  for (const [name, value] of Object.entries(document[mode]?.tokens ?? {})) {
    if (TOKEN_NAME.test(name) && typeof value === 'string' && value.trim() && !UNSAFE_VALUE.test(value)) tokens[name] = value.trim()
  }
  const properties: Record<string, string> = {}
  for (const [slot, formula] of Object.entries(DERIVED_SLOTS)) {
    if (tokens[slot]) continue
    const bases = (formula.match(/--[a-z-]+/g) ?? []).map((name) => name.slice(2))
    if (bases.some((base) => base in tokens)) properties[`--${slot}`] = formula
  }
  for (const [name, value] of Object.entries(tokens)) properties[`--${name}`] = value
  return properties
}

/** A document's CSS, or '' when it fails the last guard. */
export function documentCss(document: ThemeDocument): string {
  const css = typeof document.css === 'string' ? document.css : ''
  return UNSAFE_CSS.test(css) ? '' : css
}

/**
 * Puts a theme's CSS last in <head>, after the app's own stylesheets, so it
 * wins ties; '' removes it.
 */
export function applyThemeCss(css: string) {
  const existing = document.getElementById(THEME_STYLE_ID)
  if (!css) {
    existing?.remove()
    return
  }
  const style = existing ?? document.createElement('style')
  style.id = THEME_STYLE_ID
  if (style.textContent !== css) style.textContent = css
  if (document.head.lastElementChild !== style) document.head.appendChild(style)
}
