import { useEffect } from 'react'

/**
 * iOS keeps the page full height when the keyboard opens and pans it up
 * instead, which pushes the conversation header off-screen. On touch screens,
 * size the app to the visible area and follow the pan, so the header stays put
 * and the composer sits on the keyboard. Android resizes the page itself
 * (interactive-widget in index.html), where this changes nothing.
 */
export function useKeyboardViewport() {
  useEffect(() => {
    const viewport = window.visualViewport
    if (!viewport || typeof window.matchMedia !== 'function' || !window.matchMedia('(pointer: coarse)').matches) {
      return undefined
    }
    const root = document.documentElement
    let frame = 0
    const clear = () => {
      root.style.removeProperty('--app-height')
      root.style.removeProperty('--app-top')
      root.style.removeProperty('--app-inset-bottom')
    }
    const update = () => {
      window.cancelAnimationFrame(frame)
      frame = window.requestAnimationFrame(() => {
        // While pinch-zoomed the visible area is a magnified window, not a smaller screen.
        if (Math.abs(viewport.scale - 1) > 0.01) {
          clear()
          return
        }
        root.style.setProperty('--app-height', `${Math.round(viewport.height)}px`)
        root.style.setProperty('--app-top', `${Math.round(viewport.offsetTop)}px`)
        // The keyboard covers the home indicator, so its inset would only leave a gap.
        const keyboardOpen = root.clientHeight - viewport.height > 120
        if (keyboardOpen) root.style.setProperty('--app-inset-bottom', '0px')
        else root.style.removeProperty('--app-inset-bottom')
      })
    }
    update()
    viewport.addEventListener('resize', update)
    viewport.addEventListener('scroll', update)
    return () => {
      window.cancelAnimationFrame(frame)
      viewport.removeEventListener('resize', update)
      viewport.removeEventListener('scroll', update)
      clear()
    }
  }, [])
}
