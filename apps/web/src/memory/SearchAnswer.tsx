// Answers with sources (C2.27): the search's words as a question, answered
// from what the person can see here, each statement citing its sources.
import { useState, type ReactNode } from 'react'

import { callOp, type OpOutput } from '../api/ops'
import { Banner, Button, describeError } from '../ui'

type Answer = OpOutput<'search_answer'>

/** "[2]" in an answer, linked to its source when there is one. */
function withCitations(answer: Answer): ReactNode[] {
  const parts: ReactNode[] = []
  let last = 0
  // Brackets right after a word (`rows[1]`, quoted from a result) are an index, not a citation.
  for (const match of answer.answer.matchAll(/(?<![\p{L}\p{N}_])\[(\d{1,2})\]/gu)) {
    const at = match.index ?? 0
    if (at > last) parts.push(answer.answer.slice(last, at))
    const source = answer.sources[Number(match[1]) - 1]
    parts.push(
      source ? (
        <a className="aw-search__cite" href={source.url} key={`${at}`} title={source.title}>
          {match[1]}
        </a>
      ) : (
        match[0]
      ),
    )
    last = at + match[0].length
  }
  if (last < answer.answer.length) parts.push(answer.answer.slice(last))
  return parts
}

/** Asks for an answer on demand: a model call costs the workspace, so it never runs by itself. */
export function SearchAnswer({ projectId, query }: { projectId: string; query: string }) {
  const [asked, setAsked] = useState<{ query: string; projectId: string; answer: Answer } | null>(null)
  const [busy, setBusy] = useState(false)
  const [problem, setProblem] = useState<string | null>(null)
  // An answer belongs to the words and the place it was asked in.
  const answer = asked?.query === query && asked.projectId === projectId ? asked.answer : null
  const ask = async () => {
    setBusy(true)
    setProblem(null)
    try {
      const value = await callOp('search_answer', { question: query, ...(projectId ? { projectId } : {}) })
      setAsked({ query, projectId, answer: value })
    } catch (error) {
      setProblem(describeError(error).message)
    } finally {
      setBusy(false)
    }
  }
  if (!answer) {
    return (
      <div className="aw-search__ask">
        <Button busy={busy} icon="message" onClick={() => void ask()} size="sm" variant="secondary">
          Answer from these results
        </Button>
        {problem ? <Banner tone="danger">{problem}</Banner> : null}
      </div>
    )
  }
  return (
    <section aria-label="Answer" className="aw-search__answer">
      <p className="aw-search__answertext">{withCitations(answer)}</p>
      {answer.sources.length ? (
        <ol className="aw-search__sources">
          {answer.sources.map((source) => (
            <li key={`${source.kind}:${source.id}`}>
              <a href={source.url}>{source.title}</a>
              <span className="aw-search__sourcemeta"> · {source.project}</span>
            </li>
          ))}
        </ol>
      ) : null}
      <p className="aw-search__answernote">Written by a model from what you can see here. Check the sources.</p>
    </section>
  )
}
