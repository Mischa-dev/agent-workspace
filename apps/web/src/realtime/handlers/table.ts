import type { RealtimeEnvelope } from '../realtimeModel'
import { conversationHandlers } from './conversations'
import { outsideDataHandlers } from './outsideData'
import { peopleHandlers } from './people'
import { runHandlers } from './runs'
import { taskHandlers } from './tasks'
import type { Handler, HandlerContext, Touch } from './touches'

/**
 * What each durable event makes stale, by type (W1.3). Each module's file
 * names the keys its events touch; nothing refetches a whole project except
 * a membership change, which can change what anyone may see there.
 */
export const HANDLERS: Readonly<Record<string, Handler>> = {
  ...conversationHandlers,
  ...runHandlers,
  ...taskHandlers,
  ...peopleHandlers,
  ...outsideDataHandlers,
}

/** The queries an event makes stale. An event type the client doesn't know touches nothing. */
export function touchesFor(event: RealtimeEnvelope, context: HandlerContext): Touch[] {
  return HANDLERS[event.type]?.(event, context) ?? []
}

// Loads with the table, after the first paint: a run you asked for that finishes says so (runs/RunFinishedToasts.tsx).
export { announceRunFinished } from '../runFinished'
