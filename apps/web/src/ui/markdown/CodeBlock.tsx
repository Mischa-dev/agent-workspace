import { useEffect, useState } from 'react'

import type { Token } from './highlight'
import '../../styles/conversation.css'
import '../../styles/markdown.css'

type Highlighter = typeof import('./highlight')

let highlighter: Promise<Highlighter> | null = null

/** Loads the highlighter the first time a code block with a language appears. */
function loadHighlighter() {
  highlighter ??= import('./highlight').catch((reason: unknown) => {
    highlighter = null
    throw reason
  })
  return highlighter
}

/** A fenced code block: plain text first, coloured once the highlighter arrives. */
export function CodeBlock({ language, value }: { language: string | null; value: string }) {
  const [result, setResult] = useState<{ language: string; value: string; tokens: Token[] | null } | null>(null)
  useEffect(() => {
    if (!language) return
    let alive = true
    loadHighlighter()
      .then((module) => {
        if (alive) setResult({ language, value, tokens: module.highlight(value, language) })
      })
      .catch(() => undefined)
    return () => {
      alive = false
    }
  }, [language, value])
  const tokens = result && result.language === language && result.value === value ? result.tokens : null
  const safeLanguage = language?.replace(/[^\w+#.-]/g, '') || null
  return (
    <pre className="aw-md__pre" data-language={safeLanguage ?? undefined}>
      <code className={safeLanguage ? `language-${safeLanguage}` : undefined}>
        {tokens
          ? tokens.map((token, index) =>
              token.kind === 'plain' ? (
                token.text
              ) : (
                <span className={`aw-md-tok aw-md-tok--${token.kind}`} key={index}>
                  {token.text}
                </span>
              ),
            )
          : value}
      </code>
    </pre>
  )
}
