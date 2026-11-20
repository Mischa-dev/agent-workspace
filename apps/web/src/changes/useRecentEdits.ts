import { useEffect, useMemo, useState } from 'react'
import { useQuery } from '@tanstack/react-query'

import { opQuery } from '../api/ops'
import { formatRelative } from '../ui'
import { newestOutlined, outlines, type ChangeKind, type Outline } from './recentEdits'

/** Until changes arrive with the live events, an open editor looks again this often. */
const POLL_MS = 20_000

/**
 * A thing's recent changes and what to outline: as they land (from others,
 * the last two minutes), or all of the last day's with `all`. `version` is
 * the thing's version as the editor has it; a new one looks again at once.
 */
export function useRecentEdits(
  kind: ChangeKind,
  thingId: string,
  version: number | string | undefined,
  options: { viewerId: string; all: boolean; enabled?: boolean },
) {
  // A layout that hides recent edits (page.recent_edits) asks for none.
  const enabled = options.enabled ?? true
  const query = useQuery({ ...opQuery('change_list', { thingKind: kind, thingId, sinceHours: 24 }), refetchInterval: enabled ? POLL_MS : false, enabled })
  const { refetch } = query
  useEffect(() => {
    if (version !== undefined && enabled) void refetch()
  }, [enabled, refetch, version])
  // Outlines that landed a while ago fade out on their own.
  const [now, setNow] = useState(() => Date.now())
  useEffect(() => {
    const timer = window.setInterval(() => setNow(Date.now()), 15_000)
    return () => window.clearInterval(timer)
  }, [])
  const changes = useMemo(() => query.data?.changes ?? [], [query.data])
  const shown = useMemo(() => outlines(kind, changes, { viewerId: options.viewerId, all: options.all, now }), [changes, kind, now, options.all, options.viewerId])
  const newest = useMemo(() => newestOutlined(changes, shown), [changes, shown])
  return { changes, shown, newest }
}

/** The words an outlined item shows on hover: "Wren · 2 min ago". */
export function outlineTitle(outline: Outline) {
  return `${outline.actor} · ${formatRelative(outline.at)}`
}
