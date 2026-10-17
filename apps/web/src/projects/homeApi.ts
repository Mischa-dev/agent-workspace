import { requestJson } from '../api/client'

// Home (P3): the runs working now across the reader's projects. The rest of
// Home reads the inbox, usage, my-work and project lists.

export type WorkingRun = {
  id: string
  state: string
  projectId: string
  projectName: string
  threadId: string
  threadName: string
  task: { id: string; key: string; title: string } | null
  harness: string | null
  harnessName: string | null
  model: string | null
  startedBy: string
  mine: boolean
  approvalsWaiting: number
  createdAt: string
  startedAt: string | null
  href: string
}

export type HomeSummary = { working: WorkingRun[]; workingTotal: number; queuedLaunches: number }

export const homeKeys = { summary: ['home', 'summary'] as const }

export const getHome = () => requestJson<HomeSummary>('/api/v1/home')
