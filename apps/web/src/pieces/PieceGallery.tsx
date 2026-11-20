import { useShellPage } from '../shell/shellStore'
import { PageHeader, Section, showToast } from '../ui'
import type { Piece } from '../modules/contract'
import { moduleName } from '../modules/names'
import { PanelBlocks } from './blocks/PanelBlocks'
import { SAMPLE_PANEL } from './blocks/sample'
import { DECLARED_THINGS, EVERY_PIECE, embedsByModule } from './declared'
import { PieceView } from './Pieces'
import { RequestCard, type RequestCardData } from './RequestCard'
import { sampleThings } from './sample'

// The pieces gallery (AWD-75): every piece a module's manifest declares,
// drawn with made-up things, plus each kind of piece once. The browser guard
// (e2e/pieces.spec.ts) opens it at 1440 and 390 px in light and dark and
// fails on content cut off sideways or an axe error.

/** One piece drawn with sample things of its kind. */
export function SamplePiece({ label, piece }: { label: string; piece: Piece }) {
  const found = DECLARED_THINGS.find((entry) => entry.decl.kind === piece.thing)
  if (!found) return null
  return <PieceView decl={found.decl} label={label} piece={piece} things={sampleThings(found.decl, found.module)} />
}

/** Made-up requests, one of each state the request card draws (W1.24). */
const SAMPLE_REQUESTS: { id: string; label: string; request: RequestCardData; canAnswer?: boolean; inline?: boolean }[] = [
  {
    id: 'request.question',
    label: 'A question with suggested answers',
    request: {
      id: 'sample-question',
      kind: 'question',
      status: 'open',
      title: 'Which branch should the release notes go on?',
      detail: 'The notes are drafted. Main is frozen until Friday.',
      options: [
        { id: 'main', label: 'Main' },
        { id: 'release', label: 'The release branch' },
      ],
      allowText: true,
    },
  },
  {
    id: 'request.approval',
    label: 'An approval',
    request: {
      id: 'sample-approval',
      kind: 'approval',
      status: 'open',
      title: 'Publish the page “Release notes: 2026.11”?',
      options: [
        { id: 'approve', label: 'Approve', style: 'approve' },
        { id: 'deny', label: 'Deny', style: 'deny' },
      ],
      allowText: false,
    },
  },
  {
    id: 'request.waiting',
    label: 'Waiting on someone else',
    canAnswer: false,
    request: { id: 'sample-waiting', kind: 'question', status: 'open', title: 'Can the demo move to Thursday?', options: [], allowText: true },
  },
  {
    id: 'request.answered',
    label: 'Answered, inside an inbox item',
    inline: true,
    request: {
      id: 'sample-answered',
      kind: 'question',
      status: 'answered',
      title: 'Should the export include archived rows?',
      options: [],
      allowText: true,
      answer: { label: 'Yes', text: 'but mark them as archived' },
    },
  },
]

export function PieceGallery() {
  useShellPage({ mainClassName: 'aw-main--page' })
  const task = DECLARED_THINGS.find((entry) => entry.decl.kind === 'task')?.decl
  const taskFields = ['status', 'owner', ...(task?.fields.map((field) => field.id) ?? [])]
  return (
    <div className="aw-page">
      <PageHeader title="Pieces">
        What modules show, drawn by the app: every piece a module declares, with made-up things.
      </PageHeader>
      <Section description="Each kind of piece once, showing tasks." title="Every piece">
        {EVERY_PIECE.map((type) => (
          <div className="aw-piece-sample" data-sample={`piece.${type}`} key={type}>
            <h3 className="aw-piece-sample__title">{type}</h3>
            <SamplePiece
              label={`Sample ${type}`}
              piece={{
                type,
                thing: 'task',
                fields: type === 'form' ? ['title', ...(task?.fields.map((field) => field.id) ?? [])] : taskFields,
                groupBy: type === 'list' || type === 'board' ? 'status' : undefined,
                actions: ['open', 'edit', 'archive'],
              }}
            />
          </div>
        ))}
      </Section>
      <Section description="Someone asks; you pick an answer or type one. The same card in the inbox, a thread, a run and a task." title="Request card">
        {SAMPLE_REQUESTS.map((sample) => (
          <div className="aw-piece-sample" data-sample={sample.id} key={sample.id}>
            <h3 className="aw-piece-sample__title">{sample.label}</h3>
            <div data-piece="request">
              <RequestCard canAnswer={sample.canAnswer} request={sample.request} variant={sample.inline ? 'inline' : 'card'} />
            </div>
          </div>
        ))}
      </Section>
      <Section description="A connected module's panel: blocks it sends, drawn with these pieces. Buttons, rows and forms ask the module." title="Panel blocks">
        <div className="aw-piece-sample" data-sample="panel.sample">
          <h3 className="aw-piece-sample__title">{SAMPLE_PANEL.title}</h3>
          <PanelBlocks
            blocks={SAMPLE_PANEL.blocks}
            label={`Sample panel: ${SAMPLE_PANEL.title}`}
            onAction={(action) => showToast({ title: `The panel asked for “${action.id}”` })}
          />
        </div>
      </Section>
      {embedsByModule().map(([module, entries]) => (
        <Section description="Live blocks this module offers for pages and Home." key={module} title={moduleName(module)}>
          {entries.map(({ embed }) => (
            <div className="aw-piece-sample" data-sample={embed.id} key={embed.id}>
              <h3 className="aw-piece-sample__title">{embed.label}</h3>
              <p className="aw-piece-sample__note">{embed.description}</p>
              <SamplePiece label={embed.label} piece={embed.piece} />
            </div>
          ))}
        </Section>
      ))}
    </div>
  )
}
