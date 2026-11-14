import { useId, useLayoutEffect, useRef, useState, type KeyboardEvent, type ReactNode } from 'react'
import { Bold, Code, Italic, Link, List, ListChecks, Quote, type LucideIcon } from 'lucide-react'

import { SegmentedControl, TextArea } from '../controls'
import { Tooltip } from '../overlays'
import { Markdown } from './Markdown'
import { applyEdit, formatEdit, type FormatAction, type TextEdit } from './editing'
import '../../styles/conversation.css'
import '../../styles/markdown.css'

export type MarkdownEditorProps = {
  value: string
  onChange: (value: string) => void
  label: ReactNode
  id?: string
  hint?: ReactNode
  error?: ReactNode
  placeholder?: string
  rows?: number
  maxLength?: number
  disabled?: boolean
  name?: string
  /** Passed to the preview, see `Markdown`. */
  taskHref?: (key: string) => string | null
}

const isMac = typeof navigator !== 'undefined' && /Mac|iPhone|iPad/.test(navigator.platform || navigator.userAgent)
const mod = isMac ? '⌘' : 'Ctrl+'

const TOOLS: { action: FormatAction; label: string; icon: LucideIcon; shortcut?: string }[] = [
  { action: 'bold', label: 'Bold', icon: Bold, shortcut: `${mod}B` },
  { action: 'italic', label: 'Italic', icon: Italic, shortcut: `${mod}I` },
  { action: 'code', label: 'Code', icon: Code },
  { action: 'link', label: 'Link', icon: Link, shortcut: `${mod}K` },
  { action: 'bullet', label: 'Bulleted list', icon: List },
  { action: 'checklist', label: 'Checklist', icon: ListChecks },
  { action: 'quote', label: 'Quote', icon: Quote },
]

const SHORTCUTS: Record<string, FormatAction> = { b: 'bold', i: 'italic', k: 'link' }

/** A Markdown text area with a formatting toolbar and a preview. */
export function MarkdownEditor({
  disabled,
  error,
  hint,
  id: given,
  label,
  maxLength,
  name,
  onChange,
  placeholder,
  rows = 8,
  taskHref,
  value,
}: MarkdownEditorProps) {
  const generated = useId()
  const id = given ?? generated
  const [mode, setMode] = useState<'write' | 'preview'>('write')
  const textarea = useRef<HTMLTextAreaElement>(null)
  const pendingSelection = useRef<[number, number] | null>(null)

  useLayoutEffect(() => {
    const selection = pendingSelection.current
    const element = textarea.current
    if (!selection || !element) return
    pendingSelection.current = null
    element.focus()
    element.setSelectionRange(selection[0], selection[1])
  })

  const format = (action: FormatAction) => {
    const element = textarea.current
    if (!element || disabled) return
    const edit: TextEdit = formatEdit(action, { value, start: element.selectionStart, end: element.selectionEnd })
    const next = applyEdit(value, edit)
    if (maxLength !== undefined && next.length > maxLength) return
    element.focus()
    element.setSelectionRange(edit.from, edit.to)
    // insertText keeps the browser's undo history; fall back to a controlled update where it isn't supported.
    let inserted: boolean
    try {
      inserted = typeof document.execCommand === 'function' && document.execCommand('insertText', false, edit.insert)
    } catch {
      inserted = false
    }
    if (!inserted || element.value !== next) onChange(next)
    pendingSelection.current = [edit.selectStart, edit.selectEnd]
    if (inserted && element.value === next) {
      element.setSelectionRange(edit.selectStart, edit.selectEnd)
      pendingSelection.current = null
    }
  }

  const onKeyDown = (event: KeyboardEvent<HTMLTextAreaElement>) => {
    if (!(event.metaKey || event.ctrlKey) || event.altKey || event.shiftKey) return
    const action = SHORTCUTS[event.key.toLowerCase()]
    if (!action) return
    event.preventDefault()
    format(action)
  }

  return (
    <div className="aw-md-editor">
      <div className="aw-md-editor__bar">
        <span className="aw-field__label" id={`${id}-label`}>
          {label}
        </span>
        <div className="aw-md-editor__controls">
          {mode === 'write' ? (
            <div aria-controls={id} aria-label="Formatting" className="aw-md-editor__tools" role="toolbar">
              {TOOLS.map(({ action, icon: Glyph, label: toolLabel, shortcut }) => (
                <Tooltip key={action} label={toolLabel} shortcut={shortcut}>
                  <button
                    aria-label={toolLabel}
                    className="aw-iconbtn aw-iconbtn--sm"
                    disabled={disabled}
                    // Keep the text area's selection when a tool is clicked.
                    onMouseDown={(event) => event.preventDefault()}
                    onClick={() => format(action)}
                    type="button"
                  >
                    <Glyph aria-hidden size={16} />
                  </button>
                </Tooltip>
              ))}
            </div>
          ) : null}
          <SegmentedControl
            label="Editor mode"
            onChange={setMode}
            options={[
              { value: 'write', label: 'Write' },
              { value: 'preview', label: 'Preview' },
            ]}
            value={mode}
          />
        </div>
      </div>
      <div hidden={mode !== 'write'}>
        <TextArea
          aria-labelledby={`${id}-label`}
          className="aw-md-editor__field"
          disabled={disabled}
          error={error}
          hint={hint}
          id={id}
          maxLength={maxLength}
          name={name}
          onChange={(event) => onChange(event.target.value)}
          onKeyDown={onKeyDown}
          placeholder={placeholder}
          ref={textarea}
          rows={rows}
          value={value}
        />
      </div>
      {mode === 'preview' ? (
        <div aria-labelledby={`${id}-label`} className="aw-md-editor__preview" role="region">
          <Markdown empty={<p className="aw-md-editor__empty">Nothing to preview.</p>} source={value} taskHref={taskHref} />
        </div>
      ) : null}
    </div>
  )
}
