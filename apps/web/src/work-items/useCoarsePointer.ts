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

