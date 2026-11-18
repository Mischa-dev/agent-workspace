import { useQuery, useQueryClient } from '@tanstack/react-query'

import { callOp, opKey, opQuery, type OpInput, type OpOutput } from '../api/ops'
import type { SidebarItem, SidebarSection } from '../api/generated/operations'
import { useWorkspaceId } from '../app/workspace'
import { projectLocationHref } from '../projects/locations'

export type { SidebarItem, SidebarSection }
export type Sidebar = OpOutput<'sidebar_list'>

/** Your own sidebar sections in this workspace (C2.9): Starred, and any you named. */
export function useSidebarSections(enabled = true) {
  const workspaceId = useWorkspaceId() ?? undefined
  return useQuery({ ...opQuery('sidebar_list', {}, { workspaceId }), enabled, staleTime: 30_000 })
}

/**
 * The two writes. Each answers with the whole sidebar, which replaces the
 * one on screen; the other tabs hear `sidebar.changed`.
 */
export function useSidebarWrites() {
  const workspaceId = useWorkspaceId() ?? undefined
  const queryClient = useQueryClient()
  const keep = (sidebar: Sidebar) => {
    queryClient.setQueryData(opKey('sidebar_list', {}, workspaceId), sidebar)
    return sidebar
  }
  return {
    setSection: (input: OpInput<'sidebar_section_set'>) => callOp('sidebar_section_set', input, { workspaceId }).then(keep),
    moveItem: (input: OpInput<'sidebar_item_move'>) => callOp('sidebar_item_move', input, { workspaceId }).then(keep),
  }
}

/** Where a section's item opens: a channel in its space, a DM or group in Messages. */
export function sidebarItemHref(item: SidebarItem, href: (path: string) => string): string {
  return item.kind === 'channel'
    ? projectLocationHref(item.spaceId, { kind: 'channel', conversationId: item.conversationId })
    : href(`/messages/${encodeURIComponent(item.conversationId)}`)
}

/** The section a conversation is in, if any. */
export function sectionOf(sidebar: Sidebar | undefined, conversationId: string): SidebarSection | undefined {
  return sidebar?.sections.find((section) => section.items.some((item) => item.conversationId === conversationId))
}
