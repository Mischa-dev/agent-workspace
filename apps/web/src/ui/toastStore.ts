/** Toasts live outside React so a mutation, a realtime event or a page can show one. */
import { useSyncExternalStore } from 'react'

import type { IconName } from './icons'

export type ToastTone = 'info' | 'success' | 'danger' | 'agent'

export type ToastInput = {
  /** Reusing an id replaces that toast instead of stacking another. */
  id?: string
  title: string
  description?: string
  tone?: ToastTone
  /** A follow-up, such as "Open PR". The toast closes when it's chosen. */
  action?: { label: string; onSelect: () => void }
  /** Shows Undo. The toast closes when it's chosen. */
  onUndo?: () => void | Promise<unknown>
  /** Milliseconds before it closes on its own; 0 keeps it until dismissed. */
  duration?: number
}

export type Toast = Required<Pick<ToastInput, 'id' | 'title' | 'tone' | 'duration'>> &
  Omit<ToastInput, 'id' | 'title' | 'tone' | 'duration'> & { createdAt: number }

const MAX_TOASTS = 4
export const TOAST_ICONS: Record<ToastTone, IconName> = {
  info: 'info',
  success: 'circle-check',
  danger: 'circle-alert',
  agent: 'bot',
}

let toasts: Toast[] = []
const listeners = new Set<() => void>()
let sequence = 0

function emit() {
  for (const listener of listeners) listener()
}

function subscribe(listener: () => void) {
  listeners.add(listener)
  return () => listeners.delete(listener)
}

function snapshot() {
  return toasts
}

/** Show a toast from anywhere: a mutation, a realtime event, a page. Returns its id. */
export function showToast(input: ToastInput): string {
  sequence += 1
  const id = input.id ?? `toast-${sequence}`
  const interactive = Boolean(input.onUndo || input.action)
  const toast: Toast = {
    ...input,
    id,
    tone: input.tone ?? 'info',
    // Errors and toasts you can act on stay long enough to read and reach.
    duration: input.duration ?? (input.tone === 'danger' || interactive ? 8_000 : 4_000),
    createdAt: Date.now(),
  }
  toasts = [...toasts.filter((candidate) => candidate.id !== id), toast].slice(-MAX_TOASTS)
  emit()
  return id
}

export function dismissToast(id: string) {
  if (!toasts.some((toast) => toast.id === id)) return
  toasts = toasts.filter((toast) => toast.id !== id)
  emit()
}

/** Clears every toast. For sign-out and tests. */
export function clearToasts() {
  if (!toasts.length) return
  toasts = []
  emit()
}

export function useToasts() {
  return useSyncExternalStore(subscribe, snapshot, snapshot)
}
