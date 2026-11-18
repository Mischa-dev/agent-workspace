// Recorded event streams (W1.7): the ids a recording names. The recorder
// (e2e/support/recordFrames.ts) swaps each real id in the frames for one of
// these, so fixtures are the same on every recording and seeds can name them.

/** Ids a recording names by role. */
export const FIXTURE_IDS = {
  project: '00000000-0000-7000-8000-000000000001',
  channel: '00000000-0000-7000-8000-000000000002',
  workspace: '00000000-0000-7000-8000-000000000003',
  taskA: '00000000-0000-7000-8000-000000000011',
  taskB: '00000000-0000-7000-8000-000000000012',
  page: '00000000-0000-7000-8000-000000000021',
} as const

export type FixtureIdName = keyof typeof FIXTURE_IDS

/** Every other id, in order of first appearance: the nth is this. */
export function anonymousFixtureId(index: number) {
  return `00000000-0000-7000-8000-${(0x1000 + index).toString(16).padStart(12, '0')}`
}
