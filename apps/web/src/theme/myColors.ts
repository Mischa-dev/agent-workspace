import type { ThemeDocument } from '../api/generated/operations'
import { CONTRACT_VERSION } from './contract'
import type { ThemeChoice } from './preferences'
import { isDarkScheme } from './slots'

/** The "My colors" document for colours changed on a built-in theme. */
export function myColors(chosen: ThemeChoice, overrides: Record<string, string>): ThemeDocument {
  if (chosen === 'system')
    return { format: CONTRACT_VERSION, light: { base: 'quiet', tokens: overrides }, dark: { base: 'quiet-dark', tokens: overrides }, css: '' }
  const dark = isDarkScheme(chosen)
  return {
    format: CONTRACT_VERSION,
    light: { base: dark ? 'quiet' : chosen, tokens: dark ? {} : overrides },
    dark: { base: dark ? chosen : 'quiet-dark', tokens: dark ? overrides : {} },
    css: '',
  }
}
