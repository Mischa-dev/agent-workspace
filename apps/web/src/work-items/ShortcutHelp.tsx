import { useState } from 'react'

import { Kbd } from '../ui'
import { Popover } from '../ui/pickers'
import { TaskGlyph } from './TaskGlyph'

/**
 * A button that lists this view's shortcuts. The ? key opens the app's one
 * shortcut sheet instead, which lists these too, so only one ever opens.
 */
export function ShortcutHelp({ shortcuts }: { shortcuts: Array<{ keys: string[]; label: string }> }) {
  const [anchor, setAnchor] = useState<HTMLElement | null>(null)
  return (
    <>
      <button
        aria-expanded={anchor !== null}
        aria-haspopup="dialog"
        aria-label="Keyboard shortcuts"
        className="aw-iconbtn aw-iconbtn--md aw-tasks__help"
        onClick={(event) => setAnchor(anchor ? null : event.currentTarget)}
        title="Keyboard shortcuts"
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
