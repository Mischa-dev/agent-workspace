import { useEffect } from 'react'
import { useLocation } from 'react-router-dom'

/**
 * Brings the section named in the address (such as #models) into view once
 * the page above it has loaded. Links to single settings (#setting-…) are
 * ScopedSettings' to handle.
 */
export function useSectionScroll(ready: boolean) {
  const location = useLocation()
  useEffect(() => {
    const id = decodeURIComponent(location.hash.slice(1))
    if (!ready || !id || id.startsWith('setting-')) return
    document.getElementById(id)?.scrollIntoView?.({ block: 'start' })
  }, [location.hash, ready])
}
