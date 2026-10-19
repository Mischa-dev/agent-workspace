/** Keyboard shortcuts written the way this computer labels its keys. */

export const isMac = typeof navigator !== 'undefined' && /Mac|iPhone|iPad/.test(navigator.platform || navigator.userAgent)

const MAC: Record<string, string> = { mod: '⌘', ctrl: '⌃', alt: '⌥', shift: '⇧', enter: '↩', escape: 'Esc', esc: 'Esc', up: '↑', down: '↓', left: '←', right: '→', backspace: '⌫' }
const OTHER: Record<string, string> = { mod: 'Ctrl', ctrl: 'Ctrl', alt: 'Alt', shift: 'Shift', enter: 'Enter', escape: 'Esc', esc: 'Esc', up: '↑', down: '↓', left: '←', right: '→', backspace: 'Backspace' }

/**
 * "mod+enter" → "⌘↩" on a Mac and "Ctrl+Enter" elsewhere. "mod" is ⌘ on a
 * Mac and Ctrl everywhere else; single letters are upper-cased.
 */
export function formatShortcut(spec: string, mac = isMac): string {
  const names = mac ? MAC : OTHER
  const keys = spec.split('+').map((key) => {
    const lower = key.trim().toLowerCase()
    return names[lower] ?? (lower.length === 1 ? lower.toUpperCase() : key.trim())
  })
  return mac ? keys.join('') : keys.join('+')
}
