import path from 'node:path'

import { outputRoot } from '../environment'

/** What the tour seed made, for the tour spec to open. */
export type TourState = {
  projectId: string
  channelId: string
  channelLastMessage: string
  dmId: string
  dmLastMessage: string
  threadId: string
  threadLastMessage: string
  taskId: string
  taskTitle: string
  runId: string | null
  pageId: string
  pageTitle: string
  whiteboardId: string
  /** What the seed couldn't do but went on without; empty when it all worked. */
  warnings: string[]
}

export const tourStatePath = path.join(outputRoot, 'state', 'tour.json')
