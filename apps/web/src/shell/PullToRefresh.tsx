import { useEffect, useState, type RefObject } from 'react'
import { useQueryClient } from '@tanstack/react-query'

import { Icon } from '../ui'
import { pullDistance } from './pullModel'
import './pullToRefresh.css'

/** How far to pull, in pixels, before letting go refreshes. */
const PULL_THRESHOLD = 72

/** True when the finger is on something already scrolled down, or on a field. */
function scrolledUnder(target: Element) {
  if (target.closest('input, textarea, select, [contenteditable="true"], [role="dialog"]')) return true
  for (let element: Element | null = target; element; element = element.parentElement) {
    if (element.scrollTop > 0) return true
  }
  return window.scrollY > 0
}

export type PullState = { distance: number; refreshing: boolean }

function usePullToRefresh(ref: RefObject<HTMLElement | null>, enabled: boolean): PullState {
  const queryClient = useQueryClient()
  const [state, setState] = useState<PullState>({ distance: 0, refreshing: false })

  useEffect(() => {
    const element = ref.current
    if (!element || !enabled) return undefined
    let startY: number | null = null
    let distance = 0
    let refreshing = false

    const onStart = (event: TouchEvent) => {
      const target = event.target instanceof Element ? event.target : null
      startY = !refreshing && event.touches.length === 1 && target && !scrolledUnder(target) ? event.touches[0].clientY : null
      distance = 0
    }
    const onMove = (event: TouchEvent) => {
      if (startY === null) return
      const next = pullDistance(event.touches[0].clientY - startY)
      if (next === distance) return
      distance = next
      setState({ distance, refreshing: false })
    }
    const onEnd = () => {
      if (startY === null) return
      startY = null
      if (distance < PULL_THRESHOLD) {
        distance = 0
        setState({ distance: 0, refreshing: false })
        return
      }
      refreshing = true
      setState({ distance: PULL_THRESHOLD, refreshing: true })
      void queryClient
        .refetchQueries({ type: 'active' })
        .catch(() => undefined)
        .finally(() => {
          refreshing = false
          distance = 0
          setState({ distance: 0, refreshing: false })
        })
    }

    element.addEventListener('touchstart', onStart, { passive: true })
    element.addEventListener('touchmove', onMove, { passive: true })
    element.addEventListener('touchend', onEnd)
    element.addEventListener('touchcancel', onEnd)
    return () => {
      element.removeEventListener('touchstart', onStart)
      element.removeEventListener('touchmove', onMove)
      element.removeEventListener('touchend', onEnd)
      element.removeEventListener('touchcancel', onEnd)
    }
  }, [enabled, queryClient, ref])

  return state
}

/**
 * Pull down at the top of a page to refresh it, in the installed app on a
 * phone, where there's no reload button. AppShell loads this only there.
 * Nothing happens when anything under your finger is scrolled down.
 */
export default function PullToRefresh({ enabled, target }: { enabled: boolean; target: RefObject<HTMLElement | null> }) {
  const pull = usePullToRefresh(target, enabled)
  if (pull.distance === 0) return null
  const ready = pull.distance >= PULL_THRESHOLD
  return (
    <div
      aria-live="polite"
      className="aw-pull"
      data-ready={ready || undefined}
      role="status"
      style={{ transform: `translateY(${pull.distance - 40}px)` }}
    >
      <Icon className={pull.refreshing ? 'aw-spin' : undefined} name={pull.refreshing ? 'loader' : 'arrow-down'} size={16} />
      <span className="visually-hidden">{pull.refreshing ? 'Refreshing' : ready ? 'Release to refresh' : 'Pull to refresh'}</span>
    </div>
  )
}
