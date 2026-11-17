import { useQuery } from '@tanstack/react-query'
import { useEffect, useRef } from 'react'

import { isApiError } from '../api/errors'
import { callOp, opQuery } from '../api/ops'
import { useWorkspaceId } from '../app/workspace'
import type { ThemeDocument } from '../api/generated/operations'
import { CONTRACT_VERSION } from './contract'
import { isDarkScheme } from './slots'
import { useTheme } from './themeContext'

const GONE: ReadonlySet<string> = new Set(['not_found', 'permission_denied'])

/**
 * Keeps the custom theme in use current: a newer version (edited here, on
 * another device or by its maker, for an offered one) replaces the cached
 * copy, and a theme that's gone, or no longer offered to you, falls back to
 * the built-in one. Colours changed on a built-in theme before custom themes
 * existed become a custom theme once.
 */
export default function CustomThemeSync() {
  const theme = useTheme()
  const workspaceId = useWorkspaceId()
  const { customTheme, customDocument, refreshCustomTheme, setCustomTheme, setTheme } = theme
  const current = useQuery({
    ...opQuery('theme_get', { id: customTheme?.id ?? '' }, { workspaceId: customTheme?.workspaceId }),
    enabled: customTheme !== null,
    retry: false,
    staleTime: 60_000,
  })

  useEffect(() => {
    const found = current.data
    if (!found || !customTheme || found.id !== customTheme.id) return
    if (customDocument?.version === found.version) return
    refreshCustomTheme({ id: found.id, workspaceId: customTheme.workspaceId, version: found.version, name: found.name, document: found.document })
  }, [current.data, customTheme, customDocument?.version, refreshCustomTheme])

  useEffect(() => {
    if (customTheme && isApiError(current.error) && GONE.has(current.error.code)) setTheme(theme.theme)
  }, [current.error, customTheme, setTheme, theme.theme])

  // One-time: the old colour overrides become "My colours", on the theme they were made for.
  const converting = useRef(false)
  const overrides = theme.overrides
  useEffect(() => {
    if (converting.current || customTheme || !workspaceId || !Object.keys(overrides).length || theme.theme === 'system') return
    converting.current = true
    const dark = isDarkScheme(theme.theme)
    const document: ThemeDocument = {
      format: CONTRACT_VERSION,
      light: { base: dark ? 'quiet' : theme.theme, tokens: dark ? {} : overrides },
      dark: { base: dark ? theme.theme : 'quiet-dark', tokens: dark ? overrides : {} },
      css: '',
    }
    void callOp('theme_create', { name: 'My colours', document }, { workspaceId })
      .then((created) => {
        setCustomTheme({ id: created.id, workspaceId, version: created.version, name: created.name, document: created.document })
        theme.setMode(dark ? 'dark' : 'light')
      })
      .catch(() => {
        // Leave the colours as they are; the next visit tries again.
      })
  }, [customTheme, overrides, setCustomTheme, theme, workspaceId])

  return null
}
