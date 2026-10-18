import { useEffect, useMemo } from 'react'
import { useLocation } from 'react-router-dom'

import { useAuthenticatedSession } from '../auth/session'
import { useDirectOverview } from '../direct/api'
import { useProjects } from '../projects/hooks'
import { loadShellLazy } from './loadShellLazy'
import { pageTitleFor, unreadCount, usePageTitleOverride } from './documentTitleModel'

const BASE_ICON = '/icons/icon.svg'

/** Draws the app icon with a dot when something is unread. Skipped where there's no canvas. */
function useFaviconBadge(count: number) {
  useEffect(() => {
    const link = document.querySelector<HTMLLinkElement>('link[rel="icon"]')
    if (!link) return undefined
    if (count === 0) {
      link.href = BASE_ICON
      return undefined
    }
    if (/jsdom/i.test(navigator.userAgent)) return undefined
    let cancelled = false
    const image = new Image()
    image.onload = () => {
      if (cancelled) return
      const canvas = document.createElement('canvas')
      canvas.width = 64
      canvas.height = 64
      const context = canvas.getContext('2d')
      if (!context) return
      context.drawImage(image, 0, 0, 64, 64)
      context.beginPath()
      context.arc(48, 16, 14, 0, Math.PI * 2)
      context.fillStyle = '#e5484d'
      context.fill()
      context.lineWidth = 4
      context.strokeStyle = '#ffffff'
      context.stroke()
      link.href = canvas.toDataURL('image/png')
    }
    image.src = BASE_ICON
    return () => {
      cancelled = true
    }
  }, [count])
}

/** The installed app's badge, where the browser supports it. */
function useAppBadge(count: number) {
  useEffect(() => {
    const badge = navigator as Navigator & {
      setAppBadge?: (count?: number) => Promise<void>
      clearAppBadge?: () => Promise<void>
    }
    if (count > 0) void badge.setAppBadge?.(count).catch(() => undefined)
    else void badge.clearAppBadge?.().catch(() => undefined)
  }, [count])
  // Signing out unmounts this without a zero render: don't leave their count behind.
  useEffect(
    () => () => {
      const badge = navigator as Navigator & { clearAppBadge?: () => Promise<void> }
      void badge.clearAppBadge?.().catch(() => undefined)
    },
    [],
  )
}

/** Remembers the page for the command menu's Recent list once you've stayed on it a moment. */
function useRememberPlace(path: string, title: string, namedByPage: boolean) {
  const actorId = useAuthenticatedSession().user.actorId
  useEffect(() => {
    if (!title) return undefined
    const timer = window.setTimeout(() => {
      void loadShellLazy().then((places) => {
        if (places.isRememberedPlace(path, title, namedByPage)) places.rememberPlace(actorId, { path, title, at: Date.now() })
      })
    }, 800)
    return () => window.clearTimeout(timer)
  }, [actorId, namedByPage, path, title])
}

/**
 * Keeps the tab title on the page you're on, with an unread count in front,
 * and the favicon and app badge in step with it.
 */
export function DocumentTitle() {
  const location = useLocation()
  const projects = useProjects()
  const direct = useDirectOverview()
  const override = usePageTitleOverride()
  const count = useMemo(
    () => unreadCount(projects.data?.projects ?? [], direct.data?.conversations ?? []),
    [direct.data?.conversations, projects.data?.projects],
  )
  const title = pageTitleFor(location.pathname, location.search, projects.data?.projects ?? [], override)

  useEffect(() => {
    const named = !title ? 'Agent Workspace' : title.endsWith('Agent Workspace') ? title : `${title} · Agent Workspace`
    document.title = `${count > 0 ? `(${count > 99 ? '99+' : count}) ` : ''}${named}`
  }, [count, title])
  useEffect(() => () => {
    document.title = 'Agent Workspace'
    const link = document.querySelector<HTMLLinkElement>('link[rel="icon"]')
    if (link) link.href = BASE_ICON
  }, [])
  useFaviconBadge(count)
  useAppBadge(count)
  useRememberPlace(location.pathname, title, Boolean(override))
  return null
}
