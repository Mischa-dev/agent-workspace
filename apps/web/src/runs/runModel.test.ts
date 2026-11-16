import { describe, expect, it } from 'vitest'

import type { RunStep } from './api'
import { activityView, causeLabel, costText, formatCost, formatTokens, hostLabel, isLiveRun, livenessLabel, logText, planMark, runStatus, splitPatch } from './runModel'

describe('formatCost', () => {
  it('says nothing when the cost is unknown, but shows a known zero', () => {
    expect(formatCost(null, 'USD')).toBeNull()
    expect(formatCost(undefined, null)).toBeNull()
    expect(formatCost(0, 'USD')).toMatch(/0\.00/)
  })

  it('keeps enough digits for small amounts and survives a bad currency', () => {
    expect(formatCost(0.0042, 'USD')).toMatch(/0\.0042/)
    expect(formatCost(1.5, 'not-a-code')).toMatch(/1\.50/)
  })
})

describe('formatTokens', () => {
  it('shortens large counts', () => {
    expect(formatTokens(950)).toBe('950')
    expect(formatTokens(12_340)).toBe('12.3k')
    expect(formatTokens(4_100_000)).toBe('4.1M')
    expect(formatTokens(Number.NaN)).toBe('0')
  })
})

describe('splitPatch', () => {
  it('names each file by its new path, and a deletion by its old one', () => {
    const patch = [
      'diff --git a/src/a.ts b/src/a.ts',
      '--- a/src/a.ts',
      '+++ b/src/a.ts',
      '@@ -1 +1 @@',
      '-old',
      '+new',
      'diff --git a/gone.txt b/gone.txt',
      'deleted file mode 100644',
      '--- a/gone.txt',
      '+++ /dev/null',
      '@@ -1 +0,0 @@',
      '-bye',
      '',
    ].join('\n')
    const files = splitPatch(patch)
    expect(files.map((file) => file.path)).toEqual(['src/a.ts', 'gone.txt'])
    expect(files[0].text).toContain('+new')
    expect(files[0].text).not.toContain('bye')
  })

  it('returns nothing for an empty patch', () => {
    expect(splitPatch('')).toEqual([])
  })
})

describe('logText', () => {
  it('includes commands, exit codes and output', () => {
    const steps: RunStep[] = [
      {
        id: '1',
        kind: 'command_completed',
        summary: 'Ran tests',
        payload: { command: 'npm test', exitCode: 1, output: 'boom\n', truncated: true },
        createdAt: '2026-10-17T10:00:00Z',
      },
    ]
    const text = logText(steps)
    expect(text).toContain('$ npm test')
    expect(text).toContain('exit 1')
    expect(text).toContain('boom')
    expect(text).toContain('(output cut short)')
  })
})

describe('run states', () => {
  it('labels the run record\'s states and keeps the built-in ones', () => {
    expect(runStatus('cancel_requested').label).toBe('Stopping…')
    expect(runStatus('start_uncertain').tone).toBe('warning')
    expect(runStatus('done').label).toBe('Done')
    expect(runStatus('completed').label).toBe('Done')
    expect(runStatus('blocked').label).toBe('Waiting on you')
    expect(isLiveRun('waiting')).toBe(true)
    expect(isLiveRun('refused')).toBe(false)
  })
})

describe('livenessLabel', () => {
  const now = new Date('2026-11-16T10:00:00Z').getTime()
  it('says how long a run has been silent, only while it runs', () => {
    expect(livenessLabel({ liveness: 'ok', state: 'working', lastReportAt: null }, now)).toBeNull()
    expect(livenessLabel({ liveness: 'not_responding', state: 'working', lastReportAt: '2026-11-16T09:48:00Z' }, now)).toEqual({
      label: 'Not responding for 12 min',
      tone: 'danger',
    })
    expect(livenessLabel({ liveness: 'quiet', state: 'waiting', lastReportAt: '2026-11-16T09:55:00Z' }, now)?.label).toBe('Quiet for 5 min')
    expect(livenessLabel({ liveness: 'not_responding', state: 'done', lastReportAt: null }, now)).toBeNull()
  })
})

describe('costText', () => {
  it('marks a cost estimated from usage windows', () => {
    expect(costText(1.2, 'USD', 'estimated')).toEqual({ text: expect.stringMatching(/^≈ .*1\.20/), hint: 'Estimated from usage windows' })
    expect(costText(1.2, 'USD', 'exact')?.hint).toBeNull()
    expect(costText(null, 'USD', 'exact')).toBeNull()
  })
})

describe('causeLabel and hostLabel', () => {
  it('names why a run exists and where it runs', () => {
    expect(causeLabel({ causeKind: 'task', causeRef: null, taskKey: 'AW-12' })).toBe('AW-12')
    expect(causeLabel({ causeKind: 'schedule', causeRef: 'weekday 09:00', taskKey: null })).toBe('Scheduled: weekday 09:00')
    expect(causeLabel({ causeKind: 'host_trigger', causeRef: null, taskKey: null })).toBe('Started by its host')
    expect(hostLabel({ builtin: false, hostName: null, harnessName: null, backend: null })).toBe('Reports itself')
    expect(hostLabel({ builtin: false, hostName: 'Build box', harnessName: null, backend: 'docker' })).toBe('Build box · docker')
    expect(hostLabel({ builtin: true, hostName: null, harnessName: 'Demo', backend: null })).toBe('Demo')
  })
})

describe('activityView and planMark', () => {
  it('makes a gap stand out and reads plan marks', () => {
    const activity = { seq: 3, kind: 'gap', stepRef: null, parentRef: null, summary: '', payload: {}, occurredAt: '2026-11-16T10:00:00Z' }
    expect(activityView(activity)).toMatchObject({ icon: 'triangle-alert', tone: 'warning' })
    expect(activityView({ ...activity, kind: 'run_finished', summary: 'Failed.', payload: { outcome: 'failed' } }).tone).toBe('danger')
    expect(planMark('done').label).toBe('Done')
    expect(planMark(undefined).label).toBe('To do')
  })
})
