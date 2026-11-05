import type { WorkItem } from './types'

/** The planning fields of a work item with nothing set, for test fixtures. */
export const EMPTY_PLANNING: Pick<
  WorkItem,
  | 'parent'
  | 'milestone'
  | 'startDate'
  | 'dueDate'
  | 'estimate'
  | 'blocked'
  | 'blockedBy'
  | 'blocks'
  | 'ready'
  | 'relations'
  | 'subtasks'
  | 'claim'
  | 'handoff'
  | 'watchers'
  | 'watching'
  | 'estimateUnit'
  | 'fieldValues'
  | 'fieldReferences'
> = {
  parent: null,
  milestone: null,
  startDate: null,
  dueDate: null,
  estimate: null,
  blocked: null,
  blockedBy: [],
  blocks: [],
  ready: true,
  relations: [],
  subtasks: [],
  claim: null,
  handoff: null,
  watchers: [],
  watching: false,
  estimateUnit: 'points',
  fieldValues: {},
  fieldReferences: [],
}
