// The panels connected modules draw in one slot (M1.14), for the shell to
// place: a space's tabs, a thing's side, the home page, settings. Each is a
// layout part of its own, `panel.<module key>.<panel>`, so layouts and roles
// hide it like any part (`experience.shows`).

import { useQuery } from '@tanstack/react-query'

import { opQuery, type OpOutput } from '../../api/ops'
import type { Experience } from '../../experience/layout'
import type { PartId } from '../../experience/parts'

export type PanelEntry = OpOutput<'module_panel_list'>['panels'][number]

export function useModulePanels(
  slot: string,
  options: { spaceId?: string; workspaceId?: string; experience?: Pick<Experience, 'shows'>; enabled?: boolean } = {},
) {
  const { spaceId, workspaceId, experience, enabled = true } = options
  const listed = useQuery({
    ...opQuery('module_panel_list', { slot, spaceId }, { workspaceId }),
    enabled,
  })
  const panels = (listed.data?.panels ?? []).filter((panel) => !experience || experience.shows(panel.part as PartId))
  return { panels, isPending: listed.isPending && enabled, error: listed.error }
}
