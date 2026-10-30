import { useSyncExternalStore } from 'react'

function subscribeCoarse(callback: () => void) {
  const query = typeof window.matchMedia === 'function' ? window.matchMedia('(pointer: coarse)') : null
  query?.addEventListener('change', callback)
  return () => query?.removeEventListener('change', callback)
}

/** Touch screens get taller rows (DESIGN.md: 40px and up). */
export function useCoarsePointer() {
  return useSyncExternalStore(
    subscribeCoarse,
    () => typeof window.matchMedia === 'function' && window.matchMedia('(pointer: coarse)').matches,
    () => false,
  )
}


function subscribePhone(callback: () => void) {
  const query = typeof window.matchMedia === 'function' ? window.matchMedia(PHONE) : null
  query?.addEventListener('change', callback)
  return () => query?.removeEventListener('change', callback)
}

const PHONE = '(max-width: 640px)'

/** A phone-width window, where task rows take two lines: the title, then key and status. */
export function usePhoneWidth() {
  return useSyncExternalStore(
    subscribePhone,
    () => typeof window.matchMedia === 'function' && window.matchMedia(PHONE).matches,
    () => false,
  )
}
