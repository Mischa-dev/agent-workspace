import {
  NO_MESSAGE_UPDATES,
  NO_REACTION_UPDATES,
  type MessageUpdates,
  type ReactionUpdates,
} from '../conversations/liveMessages'

/** Streaming text by conversation, then by message. */
export type Streams = Record<string, Record<string, string>>
/** Who is typing, by conversation, then by account: until when (ms). */
export type Typing = Record<string, Record<string, number>>
/** Who is online, by account (protocol 2's presence frames). */
export type Presence = Record<string, boolean>

/**
 * What changes many times a second while the socket is up: streaming text,
 * the latest edits and reactions, and who is typing or online. Kept outside
 * React state so that only the components reading a part of it render when
 * it changes (`useSyncExternalStore`), never everything under the provider.
 */
export type LiveState = {
  streams: Streams
  /** Messages edited or removed while connected, by message id (the latest 200). */
  messageUpdates: MessageUpdates
  /** Messages whose reactions changed while connected, by message id (the latest 200). */
  reactionUpdates: ReactionUpdates
  typing: Typing
  presence: Presence
}

export type LiveStore = {
  get: () => LiveState
  subscribe: (listener: () => void) => () => void
  update: (change: (current: LiveState) => LiveState) => void
}

export const NO_STREAMS: Streams = {}
export const NO_TYPING: Typing = {}
export const NO_PRESENCE: Presence = {}

export function createLiveStore(initial: Partial<LiveState> = {}): LiveStore {
  let state: LiveState = {
    streams: NO_STREAMS,
    messageUpdates: NO_MESSAGE_UPDATES,
    reactionUpdates: NO_REACTION_UPDATES,
    typing: NO_TYPING,
    presence: NO_PRESENCE,
    ...initial,
  }
  const listeners = new Set<() => void>()
  return {
    get: () => state,
    subscribe(listener) {
      listeners.add(listener)
      return () => {
        listeners.delete(listener)
      }
    },
    update(change) {
      const next = change(state)
      if (next === state) return
      state = next
      for (const listener of [...listeners]) listener()
    },
  }
}

/** For components rendered without a session: nothing ever changes. */
export const EMPTY_LIVE_STORE = createLiveStore()
