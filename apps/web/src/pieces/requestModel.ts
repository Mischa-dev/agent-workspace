// What a request card (W1.24) shows, apart from the piece so the shell can read it without drawing.

/** What a request card shows: a question, an approval or a proposal someone raised, and its answer once given. */
export type RequestCardData = {
  id: string
  kind: string
  /** open, answered, expired, cancelled or superseded */
  status: string
  title: string
  detail?: string | null
  options: ReadonlyArray<{ id: string; label: string; style?: string | null }>
  allowText: boolean
  answer?: { optionId?: string | null; label?: string | null; text?: string | null } | null
}

export type RequestAnswer = { optionId?: string; text?: string }

/** The words of an answer: the option picked and any words typed with it. */
export function answerText(answer: NonNullable<RequestCardData['answer']>): string {
  return [answer.label, answer.text].filter(Boolean).join(': ') || 'answered'
}

/** What became of a request that is no longer open. */
export function settledText(request: RequestCardData): string {
  if (request.status === 'answered' && request.answer) return `Answered: ${answerText(request.answer)}`
  if (request.status === 'answered') return 'Answered.'
  if (request.status === 'expired') return 'Nobody answered in time.'
  if (request.status === 'superseded') return 'A newer request replaced this one.'
  return 'This request was withdrawn.'
}
