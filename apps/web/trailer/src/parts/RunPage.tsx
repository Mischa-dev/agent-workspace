import type { ReactNode } from 'react'

import type { RunStep } from '../../../src/runs/api'
import { RunSteps } from '../../../src/runs/RunSteps'
import { Button, DetailList, Icon, PageHeader, Section, SegmentedControl, StatusChip, formatDateTime } from '../../../src/ui'
import { agents, people, project } from '../data'
import { clamp, mix, opening, progress } from '../motion'
import { Arrive } from './Chat'
import { useBoxes } from './measure'
import { Swap } from './Swap'

/*
 * Codex's run on NW-12, on the run's own page as the app draws it: what it is,
 * the files it changed, its tests and every step it took. Steps land one by
 * one and the page scrolls to keep the newest in view. Times are seconds from
 * when the page opens; the first two steps are already there.
 */

export const RUN_BRANCH = 'codex/nw-12-export-csv'
export const RUN_PR = { number: 418, title: 'Export tasks as CSV' }

const STEPS: RunStep[] = [
  {
    id: 'r1',
    kind: 'agent_note',
    summary: 'Note',
    payload: { text: 'Reading the export code, and the column order agreed in NW-9.' },
    createdAt: '2026-10-21T09:20:00Z',
  },
  { id: 'r2', kind: 'command_completed', summary: 'Searched the code', payload: { command: 'rg -n "export" src', exitCode: 0 }, createdAt: '2026-10-21T09:20:40Z' },
  {
    id: 'r3',
    kind: 'file_change',
    summary: 'Wrote the CSV encoder',
    payload: { files: [{ path: 'src/export/csv.ts', change: 'added' }] },
    createdAt: '2026-10-21T09:22:00Z',
  },
  {
    id: 'r4',
    kind: 'file_change',
    summary: 'Streamed the rows, and added the Export button',
    payload: {
      files: [
        { path: 'src/export/stream.ts', change: 'added' },
        { path: 'web/tasks/ExportButton.tsx', change: 'modified' },
      ],
    },
    createdAt: '2026-10-21T09:23:00Z',
  },
  {
    id: 'r5',
    kind: 'command_completed',
    summary: 'Ran the tests',
    payload: { command: 'pnpm test export', exitCode: 0, tests: { passed: 24, failed: 0, skipped: 0 } },
    createdAt: '2026-10-21T09:24:00Z',
  },
  {
    id: 'r6',
    kind: 'test_result',
    summary: 'Load test: 10,000 tasks exported in 1.4 seconds',
    payload: { tests: { passed: 1, failed: 0, skipped: 0 } },
    createdAt: '2026-10-21T09:25:00Z',
  },
  { id: 'r7', kind: 'run_completed', summary: `Opened pull request #${RUN_PR.number}`, payload: null, createdAt: '2026-10-21T09:26:00Z' },
]

/** When each step lands, in seconds from the page opening. */
const STEP_AT = [-2, -1, 0.5, 1.05, 1.65, 2.25, 2.95]
export const RUN_DONE_AT = STEP_AT[STEP_AT.length - 1]

const FILES = [
  { path: 'src/export/csv.ts', status: 'Added', added: 142, removed: 0, at: STEP_AT[2] },
  { path: 'src/export/stream.ts', status: 'Added', added: 96, removed: 0, at: STEP_AT[3] },
  { path: 'web/tasks/ExportButton.tsx', status: 'Changed', added: 38, removed: 6, at: STEP_AT[3] + 0.12 },
]

const PROMPT = `NW-12 Export tasks as CSV

Let people download any task view as a CSV file, with the columns in the order we agreed in NW-9.

Big projects should stream the rows instead of loading them all at once.

Done when:
- Exports what the current view shows, with its filters and sort
- Columns follow the order agreed in NW-9
- A 10,000-task project exports in under two seconds
- Dates use each person’s own time zone`

const ARRIVE = 0.45
// The page is the window's main column: this tall, and it keeps this much room under the newest step.
const VIEWPORT = 940
const ROOM = 64

const STEP_SELECTORS = Object.fromEntries([
  ['steps', '.tr-steps'],
  ...STEPS.map((_, index) => [`s${index}`, `.tr-steps .aw-runsteps__item:nth-child(${index + 1})`]),
]) as Record<string, string>

const SWAP = 0.35

/** The line under the run's title at a moment of the run. */
function summaryLine(t: number) {
  const done = t >= RUN_DONE_AT
  // Counted by step, so the line changes once for the step that wrote two files.
  const files = t >= STEP_AT[3] ? 3 : t >= STEP_AT[2] ? 1 : 0
  const minutes = 3 + STEP_AT.slice(3, 6).filter((at) => t >= at).length
  return [
    agents.codex.name,
    done ? '7 min' : `${minutes} min so far`,
    files ? `${files} ${files === 1 ? 'file' : 'files'} changed` : null,
    t >= STEP_AT[4] ? 'tests passing' : null,
    done ? '$0.41' : null,
  ]
    .filter(Boolean)
    .join(' · ')
}

// Each moment the summary line changes.
const SUMMARY_CHANGES = [...STEP_AT.slice(2, 6), RUN_DONE_AT]

function testsLine(t: number) {
  const tests = t >= STEP_AT[5] ? { runs: 2, passed: 25 } : t >= STEP_AT[4] ? { runs: 1, passed: 24 } : null
  if (!tests) return <p className="aw-runs__quiet">The run didn’t report any tests.</p>
  return (
    <p className="aw-runs__tests">
      <StatusChip tone="success">Passing</StatusChip> The last run passed. Across {tests.runs} {tests.runs === 1 ? 'test run' : 'test runs'}: {tests.passed} passed, 0
      failed, 0 skipped.
    </p>
  )
}

/** The run's status and its controls: Stop while it works, then Run again and the pull request. */
function Actions({ done }: { done: boolean }) {
  return (
    <div className="aw-runs__actions">
      <StatusChip icon={done ? 'circle-check' : 'loader'} spin={!done} tone={done ? 'success' : 'agent'}>
        {done ? 'Done' : 'Working'}
      </StatusChip>
      {done ? (
        <>
          <Button icon="retry" size="sm">
            Run again
          </Button>
          <span className="aw-btn aw-btn--secondary aw-btn--sm">
            <Icon name="pull-request" size={14} />
            <span className="aw-btn__label">Open pull request</span>
          </span>
        </>
      ) : (
        <Button icon="stop" size="sm">
          Stop run
        </Button>
      )}
    </div>
  )
}

const link = (children: ReactNode) => (
  <a href="#run" onClick={(event) => event.preventDefault()}>
    {children}
  </a>
)

export function RunPage({ t }: { t: number }) {
  const m = useBoxes('run-page', STEP_SELECTORS)
  const shown = STEP_AT.filter((at) => t >= at).length
  const k = shown ? progress(t, STEP_AT[shown - 1], ARRIVE) : 0

  // Every step is laid out from the start, hidden until it lands, so the list can open smoothly to each new one.
  const top = m.box('steps').y
  const bottom = (count: number) => {
    if (count <= 0) return 0
    const box = m.box(`s${count - 1}`)
    return box.y + box.h - top
  }
  // The list opens to each new step on the in-out curve, and the page scrolls with it.
  const height = mix(bottom(shown - 1), bottom(shown), opening(k))
  const scroll = m.ready('steps') ? Math.max(0, top + height + ROOM - VIEWPORT) : 0

  // Whatever changes as steps land (the summary line, the tests, the status) cross-fades into its new state.
  const changed = (at: number) => progress(t, at, SWAP)
  const lastChange = (times: number[]) => times.filter((at) => t >= at).pop() ?? -Infinity
  const summaryAt = lastChange(SUMMARY_CHANGES)
  const testsAt = lastChange([STEP_AT[4], STEP_AT[5]])
  const doneK = changed(RUN_DONE_AT)

  const details: { label: string; value: ReactNode }[] = [
    { label: 'Project', value: project.name },
    { label: 'Agent', value: agents.codex.name },
    { label: 'Harness', value: 'Codex' },
    { label: 'Model', value: 'Harness default' },
    { label: 'Task', value: link('NW-12 Export tasks as CSV') },
    { label: 'Thread', value: link('Export tasks as CSV') },
    { label: 'Branch', value: <code>{RUN_BRANCH}</code> },
    { label: 'Requested by', value: people.priya.name },
    { label: 'Requested', value: <time dateTime="2026-10-21T09:19:00Z">{formatDateTime('2026-10-21T09:19:00Z')}</time> },
    // Last, so it fills the empty cell beside Requested when it arrives and nothing else moves.
    { label: 'Pull request', value: link(`#${RUN_PR.number} · Open`) },
  ]

  const files = FILES.filter((file) => t >= file.at)
  const firstFile = files.length ? progress(t, FILES[0].at, ARRIVE) : 0
  const hiddenSteps = `.tr-steps .aw-runsteps__item:nth-child(n + ${shown + 1}) { visibility: hidden }
.tr-steps .aw-runsteps__item:nth-child(${Math.max(1, shown)}) { opacity: ${shown ? clamp(k * 1.4) : 1}; transform: translate3d(0, ${(1 - k) * 12}px, 0) }
.tr-rundetails .aw-dl__row:nth-child(${details.length}) { opacity: ${doneK}; transform: translate3d(0, ${(1 - doneK) * 6}px, 0) }`

  return (
    <div className="aw-page aw-runs" ref={m.root} style={{ position: 'relative', transform: `translate3d(0, ${-scroll}px, 0)` }}>
      <span className="aw-btn aw-btn--ghost aw-btn--sm aw-runs__back">
        <Icon name="arrow-left" size={14} />
        <span className="aw-btn__label">All runs</span>
      </span>
      <PageHeader
        actions={<Swap align="end" from={<Actions done={false} />} id="run-actions" inline k={doneK} to={<Actions done />} />}
        icon="activity"
        title="NW-12 Export tasks as CSV"
      >
        <Swap from={summaryLine(summaryAt - 0.001)} id="run-summary" k={changed(summaryAt)} to={summaryLine(summaryAt)} />
      </PageHeader>
      <div className="aw-runs__details tr-rundetails">
        <DetailList items={details} />
      </div>
      <Section id="run-files" title="Files changed">
        {/* One box for all of it, so the section's gaps don't appear and vanish as its parts do. */}
        <div className="tr-runfiles">
          <Arrive id="run-files-quiet" k={1} out={firstFile}>
            <p className="aw-runs__quiet">No files changed yet. They show up here as Codex edits them.</p>
          </Arrive>
          {files.length ? (
            <ul className="aw-rundiff__files" style={{ marginBottom: `calc(var(--space-3) * ${opening(firstFile)})` }}>
              {files.map((file) => (
                <Arrive id={`run-file-${file.path}`} k={progress(t, file.at, ARRIVE)} key={file.path}>
                  <li className="aw-rundiff__file">
                    <code className="aw-rundiff__path">{file.path}</code>
                    <span className="aw-rundiff__stat">
                      {file.status}
                      {' · '}
                      <span className="aw-rundiff__added">+{file.added}</span> <span className="aw-rundiff__removed">−{file.removed}</span>
                    </span>
                  </li>
                </Arrive>
              ))}
            </ul>
          ) : null}
          <Arrive id="run-diff-button" k={firstFile}>
            <div className="tr-runfiles__diff">
              <Button icon="branch">Show the diff</Button>
            </div>
          </Arrive>
        </div>
      </Section>
      <Section id="run-tests" title="Tests">
        <Swap from={testsLine(testsAt - 0.001)} id="run-tests" k={changed(testsAt)} to={testsLine(testsAt)} />
      </Section>
      <Section
        actions={
          <div className="aw-runs__actions">
            <SegmentedControl
              label="Show as"
              onChange={() => undefined}
              options={[
                { value: 'steps', label: 'Steps' },
                { value: 'log', label: 'Log' },
              ]}
              value="steps"
            />
          </div>
        }
        id="run-steps"
        title="What it did"
      >
        <div className="tr-steps" style={{ position: 'relative', height, overflow: 'hidden' }}>
          <style>{hiddenSteps}</style>
          <RunSteps hidden={0} steps={STEPS} />
        </div>
      </Section>
      <Section id="run-prompt" title="What it was asked">
        <pre className="aw-runs__pre aw-runs__prompt">{PROMPT}</pre>
      </Section>
    </div>
  )
}
