import { createContext } from 'react'

import { isSafeMessageText } from './messageContentModel'
import type { ForwardedMessage } from './types'

function text(value: unknown) {
  return typeof value === 'string' ? value : null
}

/** The wire's forward, checked: only what's available is kept, and only in-app links. */
export function readForwarded(value: unknown): ForwardedMessage | null {
  const forwarded = value && typeof value === 'object' && !Array.isArray(value) ? (value as Record<string, unknown>) : {}
  if (typeof forwarded.messageId !== 'string') return null
  const available = forwarded.available === true
  const href = text(forwarded.href)
  const quote = text(forwarded.text)
  return {
    messageId: forwarded.messageId,
    available,
    removed: available && forwarded.removed === true,
    authorName: available ? text(forwarded.authorName) : null,
    place: available ? text(forwarded.place) : null,
    text: available && quote !== null && isSafeMessageText(quote) ? quote.slice(0, 2_000) : null,
    createdAt: available ? text(forwarded.createdAt) : null,
    href: available && href?.startsWith('/') && !href.startsWith('//') ? href : null,
  }
}

/** Scroll to a message on this page and flash it, the way a copied link does. False when it isn't here. */
export function revealMessage(messageId: string) {
  // Task comments (AWD-95) are replied to the same way as messages.
  const element = document.getElementById(`message-${messageId}`) ?? document.getElementById(`comment-${messageId}`)
  if (!element) return false
  element.scrollIntoView({ block: 'center', behavior: 'smooth' })
  element.classList.add('is-linked')
  window.setTimeout(() => element.classList.remove('is-linked'), 2600)
  return true
}

/**
 * Set by a thread open beside its conversation: finds a message in the
 * thread, loading earlier pages until it's there, instead of following the
 * original's link, which would close the thread.
 */
export const RevealInThread = createContext<((messageId: string) => void) | null>(null)
