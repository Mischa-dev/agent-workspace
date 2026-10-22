import type { ReactNode } from 'react'

import { clamp } from './motion'
import { Agents } from './scenes/Agents'
import { End } from './scenes/End'
import { Features } from './scenes/Features'
import { Hook } from './scenes/Hook'
import { Next } from './scenes/Next'
import { Graph } from './scenes/Graph'
import { Plan } from './scenes/Plan'
import { Runs } from './scenes/Runs'
import { Task } from './scenes/Task'
import { Team } from './scenes/Team'
import { Title } from './scenes/Title'

/*
 * The cut. Times are seconds; the soundtrack is 120 beats a minute, so a bar
 * is two seconds and most scene changes land on a bar. Neighbouring scenes
 * overlap a little where one hands over to the next, and the backdrop
 * cross-fades when the tone changes from dark to light.
 */

type Scene = {
  id: string
  start: number
  end: number
  tone: 'dark' | 'light'
  render: (t: number, length: number) => ReactNode
}

export const SCENES: Scene[] = [
  { id: 'hook', start: 0, end: 7.2, tone: 'dark', render: (t) => <Hook t={t} /> },
  { id: 'title', start: 6.75, end: 12, tone: 'dark', render: (t, length) => <Title end={length} t={t} /> },
  // The task scene picks up the plan's window as NW-12 opens, so the two meet on one frame instead of overlapping.
  { id: 'plan', start: 11.6, end: 19.6, tone: 'light', render: (t) => <Plan t={t} /> },
  { id: 'task', start: 19.6, end: 28.4, tone: 'light', render: (t) => <Task t={t} /> },
  { id: 'graph', start: 28.0, end: 34.4, tone: 'light', render: (t) => <Graph t={t} /> },
  { id: 'team', start: 34.0, end: 50.0, tone: 'light', render: (t) => <Team t={t} /> },
  { id: 'features', start: 49.6, end: 57.0, tone: 'light', render: (t) => <Features t={t} /> },
  { id: 'runs', start: 56.6, end: 67.2, tone: 'light', render: (t) => <Runs t={t} /> },
  { id: 'agents', start: 66.8, end: 76.0, tone: 'light', render: (t) => <Agents t={t} /> },
  { id: 'next', start: 75.6, end: 83.0, tone: 'light', render: (t) => <Next t={t} /> },
  { id: 'end', start: 82.6, end: 90.0, tone: 'dark', render: (t) => <End t={t} /> },
]

export const DURATION = Math.max(...SCENES.map((scene) => scene.end))

const FADE = 0.5

// Runs of light scenes that touch, so one light scene handing over to the next never dims the backdrop.
const LIGHT_RUNS = SCENES.filter((scene) => scene.tone === 'light').reduce<{ start: number; end: number }[]>((runs, scene) => {
  const last = runs[runs.length - 1]
  if (last && scene.start <= last.end) last.end = Math.max(last.end, scene.end)
  else runs.push({ start: scene.start, end: scene.end })
  return runs
}, [])

/** How much of the light backdrop shows: it fades in where a run of light scenes starts and out where it ends. */
function lightness(t: number) {
  let light = 0
  for (const run of LIGHT_RUNS) {
    light = Math.max(light, Math.min(clamp((t - run.start) / FADE), clamp((run.end - t) / FADE)))
  }
  return light
}

export function Trailer({ t }: { t: number }) {
  return (
    <div className="tr-stage">
      <div className="tr-backdrop tr-backdrop--dark" />
      <div className="tr-backdrop tr-backdrop--light" style={{ opacity: lightness(t) }} />
      {SCENES.filter((scene) => t >= scene.start && t < scene.end).map((scene) => (
        <section className={`tr-scene tr-scene--${scene.tone}`} data-scene={scene.id} key={scene.id}>
          {scene.render(t - scene.start, scene.end - scene.start)}
        </section>
      ))}
    </div>
  )
}
