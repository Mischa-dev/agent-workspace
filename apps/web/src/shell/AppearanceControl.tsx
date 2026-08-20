import { useEffect, useRef, useState } from 'react'

import { useTheme } from '../theme/themeContext'
import { Popover } from '../ui/primitives'

export function AppearanceControl() {
  const [open, setOpen] = useState(false)
  const triggerRef = useRef<HTMLButtonElement>(null)
  const panelRef = useRef<HTMLDivElement>(null)
  const theme = useTheme()

  useEffect(() => {
    if (!open) return
    panelRef.current?.querySelector<HTMLElement>('select, button')?.focus()
    const close = (event: KeyboardEvent) => {
      if (event.key !== 'Escape') return
      event.stopPropagation()
      setOpen(false)
      window.requestAnimationFrame(() => triggerRef.current?.focus())
    }
    document.addEventListener('keydown', close)
    return () => document.removeEventListener('keydown', close)
  }, [open])

  return (
    <div className="appearance-control" ref={panelRef}>
      <button
        aria-expanded={open}
        aria-haspopup="dialog"
        aria-label="Appearance and density"
        className="project-button"
        onClick={() => setOpen((current) => !current)}
        ref={triggerRef}
        type="button"
      >
        <span aria-hidden="true">◐</span>
      </button>
      <Popover label="Appearance and density" open={open}>
        <header>
          <strong>Appearance</strong>
          <button aria-label="Close appearance" className="icon-button" onClick={() => { setOpen(false); window.requestAnimationFrame(() => triggerRef.current?.focus()) }} type="button">×</button>
        </header>
        <label>
          Theme
          <select onChange={(event) => theme.setTheme(event.target.value as 'system' | 'dark' | 'light')} value={theme.theme}>
            <option value="system">System</option>
            <option value="dark">Dark</option>
            <option value="light">Light</option>
          </select>
        </label>
        <label>
          Density
          <select onChange={(event) => theme.setDensity(event.target.value as 'compact' | 'comfortable' | 'spacious')} value={theme.density}>
            <option value="compact">Compact</option>
            <option value="comfortable">Comfortable</option>
            <option value="spacious">Spacious</option>
          </select>
        </label>
        <p>{theme.resolvedTheme === 'dark' ? 'Dark' : 'Light'} palette active. Preferences stay on this device.</p>
        <button className="text-button" onClick={theme.resetAppearance} type="button">Reset appearance</button>
      </Popover>
    </div>
  )
}
