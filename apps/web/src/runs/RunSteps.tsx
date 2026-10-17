import { useState } from 'react'

import { Button, Icon, formatTime } from '../ui'
import type { RunStep } from './api'
import { stepView } from './runModel'

/** The run's steps in order: commands, file changes, notes, tools, approvals and tests. */
export function RunSteps({ hidden, steps }: { hidden: number; steps: RunStep[] }) {
  if (steps.length === 0) return <p className="aw-runs__quiet">The run hasn’t done anything yet.</p>
  return (
    <>
      {hidden > 0 ? (
        <p className="aw-runs__quiet">
          {hidden} earlier {hidden === 1 ? 'step isn’t' : 'steps aren’t'} shown.
        </p>
      ) : null}
      <ol className="aw-runsteps">
        {steps.map((step) => (
          <StepItem key={step.id} step={step} />
        ))}
      </ol>
    </>
  )
}

function StepItem({ step }: { step: RunStep }) {
  const view = stepView(step)
  const payload = step.payload ?? {}
  const exitCode = typeof payload.exitCode === 'number' ? payload.exitCode : null
  return (
    <li className={`aw-runsteps__item aw-runsteps__item--${view.tone}`}>
      <span aria-hidden className="aw-runsteps__icon">
        <Icon name={view.icon} size={14} />
      </span>
      <div className="aw-runsteps__body">
        <div className="aw-runsteps__head">
          <span className={payload.command ? 'aw-runsteps__title aw-runsteps__title--code' : 'aw-runsteps__title'}>{view.title}</span>
          <time className="aw-runsteps__time" dateTime={step.createdAt}>
            {formatTime(step.createdAt)}
          </time>
        </div>
        {exitCode !== null ? (
          <p className="aw-runsteps__meta">{exitCode === 0 ? 'Exited with 0' : `Failed with exit code ${exitCode}`}</p>
        ) : null}
        {payload.tests ? (
          <p className="aw-runsteps__meta">
            {payload.tests.passed ?? 0} passed, {payload.tests.failed ?? 0} failed, {payload.tests.skipped ?? 0} skipped
          </p>
        ) : null}
        {payload.files?.length ? (
          <ul className="aw-runsteps__files">
            {payload.files.map((file) => (
              <li key={`${file.change ?? ''}${file.path}`}>
                <code>{file.path}</code>
                {file.change ? <span className="aw-runsteps__meta"> {file.change}</span> : null}
              </li>
            ))}
          </ul>
        ) : null}
        {payload.text && payload.text !== view.title ? <p className="aw-runsteps__text">{payload.text}</p> : null}
        {payload.output ? <StepOutput output={payload.output} truncated={payload.truncated === true} /> : null}
      </div>
    </li>
  )
}

function StepOutput({ output, truncated }: { output: string; truncated: boolean }) {
  const [open, setOpen] = useState(false)
  const lines = output.split('\n').length
  return (
    <div className="aw-runsteps__output">
      <Button aria-expanded={open} icon={open ? 'chevron-down' : 'chevron-right'} onClick={() => setOpen(!open)} size="sm" variant="ghost">
        {open ? 'Hide output' : `Show output (${lines} ${lines === 1 ? 'line' : 'lines'})`}
      </Button>
      {open ? (
        <>
          <pre className="aw-runs__pre">{output}</pre>
          {truncated ? <p className="aw-runsteps__meta">The output was cut short.</p> : null}
        </>
      ) : null}
    </div>
  )
}
