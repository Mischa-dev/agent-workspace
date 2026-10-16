import { useEffect, useRef, useState } from 'react'

import { Kbd, Popover } from '../ui'
import { isTypingTarget } from './shortcutKeys'
import { TaskGlyph } from './TaskGlyph'

/** A "?" button that lists the shortcuts. The ? key opens it too. */
export function ShortcutHelp({ shortcuts }: { shortcuts: Array<{ keys: string[]; label: string }> }) {
  const [anchor, setAnchor] = useState<HTMLElement | null>(null)
  const button = useRef<HTMLButtonElement>(null)
  useEffect(() => {
    const onKeyDown = (event: KeyboardEvent) => {
      if (event.key !== '?' || isTypingTarget(event.target) || document.querySelector('.aw-dialog-layer')) return
      event.preventDefault()
      setAnchor((current) => (current ? null : button.current))
    }
    window.addEventListener('keydown', onKeyDown)
    return () => window.removeEventListener('keydown', onKeyDown)
  }, [])
  return (
    <>
      <button
        aria-expanded={anchor !== null}
        aria-haspopup="dialog"
        aria-label="Keyboard shortcuts"
        className="aw-iconbtn aw-iconbtn--md aw-tasks__help"
        onClick={(event) => setAnchor(anchor ? null : event.currentTarget)}
        ref={button}
        title="Keyboard shortcuts (?)"
        type="button"
      >
        <TaskGlyph name="keyboard" size={18} />
      </button>
      <Popover anchor={anchor} label="Keyboard shortcuts" onClose={() => setAnchor(null)} open={anchor !== null} placement="bottom-end" width={320}>
        <div className="aw-shortcuts" tabIndex={-1}>
          <h2 className="aw-shortcuts__title">Keyboard shortcuts</h2>
          <dl className="aw-shortcuts__list">
            {shortcuts.map((shortcut) => (
              <div className="aw-shortcuts__row" key={shortcut.label}>
                <dt>
                  {shortcut.keys.map((key) => (
                    <Kbd key={key}>{key}</Kbd>
                  ))}
                </dt>
                <dd>{shortcut.label}</dd>
              </div>
            ))}
          </dl>
        </div>
      </Popover>
    </>
  )
}
