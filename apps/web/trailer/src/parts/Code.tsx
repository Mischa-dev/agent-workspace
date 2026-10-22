import type { CSSProperties } from 'react'

import { type as typeStyle } from '../motion'

/*
 * A code editor card, dark like the app's run log, that types its code out.
 * Each line is a list of runs: [token class, text]. The class is one of the
 * trailer's tr-tok-* colors, or '' for plain text.
 */

export type Run = [string, string]
export type CodeLine = Run[]

const length = (lines: CodeLine[]) => lines.reduce((sum, line) => sum + line.reduce((n, [, text]) => n + text.length, 0) + 1, 0)

export function codeLength(lines: CodeLine[]) {
  return length(lines)
}

export function CodeCard({
  file,
  lines,
  scale = 1.2,
  shown,
  style,
  terminal,
  terminalShown = [],
  typing,
}: {
  file: string
  lines: CodeLine[]
  scale?: number
  /** How many characters have been typed, counting a newline at the end of each line. */
  shown: number
  style?: CSSProperties
  terminal?: CodeLine[]
  /** How far each terminal line has faded in, as output appears (1 where left out). */
  terminalShown?: number[]
  /** Holds the caret steady while keys are going in. */
  typing?: boolean
}) {
  let left = shown
  const done = shown >= length(lines)
  return (
    <div className="tr-code" style={style}>
      <div className="tr-code__bar">
        <span className="tr-code__dot" />
        <span className="tr-code__dot" />
        <span className="tr-code__dot" />
        <span className="tr-code__file" style={typeStyle('--t-code-sm', scale)}>
          {file}
        </span>
      </div>
      <pre className="tr-code__body" style={typeStyle('--t-code', scale)}>
        {lines.map((line, index) => {
          const total = line.reduce((n, [, text]) => n + text.length, 0)
          const take = Math.max(0, Math.min(total, left))
          const caretHere = !done && left >= 0 && left <= total
          left -= total + 1
          let budget = take
          return (
            <span className="tr-code__line" key={index}>
              {line.map(([token, text], part) => {
                const piece = text.slice(0, Math.max(0, budget))
                budget -= text.length
                return piece ? (
                  <span className={token || undefined} key={part}>
                    {piece}
                  </span>
                ) : null
              })}
              {caretHere ? <span className={typing ? 'tr-caret tr-caret--code is-typing' : 'tr-caret tr-caret--code'} /> : null}
              {'\n'}
            </span>
          )
        })}
      </pre>
      {terminal?.length ? (
        <pre className="tr-code__terminal" style={typeStyle('--t-code', scale)}>
          {terminal.map((line, index) => (
            <span className="tr-code__line" key={index} style={{ opacity: terminalShown[index] ?? 1 }}>
              {line.map(([token, text], part) => (
                <span className={token || undefined} key={part}>
                  {text}
                </span>
              ))}
              {'\n'}
            </span>
          ))}
        </pre>
      ) : null}
    </div>
  )
}
