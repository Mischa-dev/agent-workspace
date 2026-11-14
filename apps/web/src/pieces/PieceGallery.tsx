import { useAuthenticatedSession } from '../auth/session'
import { AppShell } from '../shell/AppShell'
import { PageHeader, Section } from '../ui'
import type { Piece } from '../modules/contract'
import { moduleName } from '../modules/names'
import { DECLARED_THINGS, EVERY_PIECE, embedsByModule } from './declared'
import { PieceView } from './Pieces'
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

export function PieceGallery() {
  const session = useAuthenticatedSession()
  const task = DECLARED_THINGS.find((entry) => entry.decl.kind === 'task')?.decl
  const taskFields = ['status', 'owner', ...(task?.fields.map((field) => field.id) ?? [])]
  return (
    <AppShell mainClassName="aw-main--page" user={session.user}>
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
    </AppShell>
  )
}
