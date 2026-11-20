import { useMutation, useQuery, useQueryClient } from '@tanstack/react-query'
import { useState } from 'react'

import { callOp, type OpInput } from '../api/ops'
import { queryKeys } from '../api/queryKeys'
import { chartNumber, chartTicks } from '../collections/collectionModel'
import { Banner, Button, IconButton, Section, SelectField, Switch, TextField, describeError, showToast } from '../ui'
import { formatDateLabel } from '../ui/dateModel'
import { burnUp, cycleDates, cycleLengths, type Cycle, lengthLabel, linePath, progressLabel, weekdays } from './cycleModel'
import type { WorkItemWorkflow } from './types'

const cyclesKey = (projectId: string) => ['project', projectId, 'cycles'] as const

type Change = Omit<OpInput<'cycle_set'>, 'spaceId'>

/**
 * A space's cycles (T1.24): fixed-length periods of work, made ahead by a
 * daily job that closes each one as it ends, rolls its unfinished tasks
 * into the next and counts the running one's tasks for its burn-up.
 */
export function CycleSettings({ projectId, workflow }: { projectId: string; workflow: WorkItemWorkflow }) {
  const client = useQueryClient()
  const cycles = useQuery({ queryKey: cyclesKey(projectId), queryFn: () => callOp('cycle_list', { spaceId: projectId }) })
  const [renaming, setRenaming] = useState<string | null>(null)
  const [shown, setShown] = useState<string | null>(null)
  const write = useMutation({
    mutationFn: (change: Change) => callOp('cycle_set', { spaceId: projectId, ...change }, { requestId: crypto.randomUUID() }),
    onSuccess: (written) => {
      client.setQueryData(cyclesKey(projectId), written)
      void client.invalidateQueries({ queryKey: queryKeys.workItemsRoot(projectId) })
    },
    onError: (error) => showToast({ tone: 'danger', title: describeError(error).message }),
  })
  const settings = cycles.data?.settings
  const canEdit = workflow.canEdit
  const change = (next: Change) => write.mutate({ ...next, expectedVersion: settings?.version })
  const list = cycles.data?.cycles ?? []
  const running = list.find((cycle) => cycle.state === 'running')
  const charted = list.find((cycle) => cycle.id === shown) ?? running ?? list.filter((cycle) => cycle.state === 'ended').at(-1)

  return (
    <Section
      description="Plan work in fixed-length periods. Upcoming cycles are made ahead; when one ends, its unfinished tasks can move into the next."
      id="workflow-cycles-heading"
      title="Cycles"
    >
      {cycles.isError ? <Banner tone="danger">{describeError(cycles.error).message}</Banner> : null}
      {settings && !settings.enabled ? (
        canEdit ? (
          <Button busy={write.isPending} onClick={() => change({ enabled: true })}>
            Turn on cycles
          </Button>
        ) : (
          <p className="aw-triagerules__note">This space doesn’t use cycles.</p>
        )
      ) : null}
      {settings?.enabled ? (
        <>
          <div className="aw-cycles__settings">
            <SelectField disabled={!canEdit || write.isPending} label="Length" onChange={(event) => change({ lengthDays: Number(event.target.value) })} value={settings.lengthDays}>
              {cycleLengths.map((days) => (
                <option key={days} value={days}>
                  {lengthLabel(days)}
                </option>
              ))}
            </SelectField>
            <SelectField
              disabled={!canEdit || write.isPending}
              label="Starts on"
              onChange={(event) => change({ startsOnWeekday: Number(event.target.value) })}
              value={settings.startsOnWeekday}
            >
              {weekdays.map((day, index) => (
                <option key={day} value={index + 1}>
                  {day}
                </option>
              ))}
            </SelectField>
            <SelectField disabled={!canEdit || write.isPending} label="Made ahead" onChange={(event) => change({ upcoming: Number(event.target.value) })} value={settings.upcoming}>
              {[1, 2, 3, 4, 5, 6].map((count) => (
                <option key={count} value={count}>
                  {count}
                </option>
              ))}
            </SelectField>
            <Switch checked={settings.rollOver} disabled={!canEdit || write.isPending} label="Move unfinished tasks into the next cycle" onChange={(on) => change({ rollOver: on })} />
            {canEdit ? (
              <Button busy={write.isPending} onClick={() => change({ enabled: false })} size="sm" variant="ghost">
                Turn off
              </Button>
            ) : null}
          </div>
          <p className="aw-triagerules__note">Length and start day apply to cycles made from now on.</p>
        </>
      ) : null}
      {list.length ? (
        <ol className="aw-wf__entries aw-cycles">
          {list.map((cycle) => (
            <li className="aw-cycles__cycle" data-state={cycle.state} key={cycle.id}>
              {renaming === cycle.id ? (
                <RenameForm
                  busy={write.isPending}
                  cycle={cycle}
                  onCancel={() => setRenaming(null)}
                  onSave={(name) => write.mutate({ cycle: cycle.id, name }, { onSuccess: () => setRenaming(null) })}
                />
              ) : (
                <>
                  <button aria-pressed={charted?.id === cycle.id} className="aw-cycles__pick" onClick={() => setShown(cycle.id)} type="button">
                    <strong>{cycle.title}</strong>
                    <span>
                      {cycleDates(cycle)} · {stateLabel(cycle.state)} · {progressLabel(cycle)}
                    </span>
                  </button>
                  {canEdit ? <IconButton disabled={write.isPending} icon="pencil" label={`Rename ${cycle.title}`} onClick={() => setRenaming(cycle.id)} size="sm" /> : null}
                </>
              )}
            </li>
          ))}
        </ol>
      ) : null}
      {charted ? <BurnUp cycle={charted} /> : null}
    </Section>
  )
}

function stateLabel(state: string) {
  return state === 'running' ? 'Now' : state === 'upcoming' ? 'Upcoming' : 'Ended'
}

function RenameForm({ busy, cycle, onCancel, onSave }: { busy: boolean; cycle: Cycle; onCancel: () => void; onSave: (name: string) => void }) {
  const [name, setName] = useState(cycle.name)
  return (
    <form
      className="aw-cycles__rename"
      onSubmit={(event) => {
        event.preventDefault()
        onSave(name.trim())
      }}
    >
      <TextField autoFocus label={`Name for cycle ${cycle.number}`} maxLength={80} onChange={(event) => setName(event.target.value)} placeholder={`Cycle ${cycle.number}`} value={name} />
      <Button onClick={onCancel} size="sm" variant="ghost">
        Cancel
      </Button>
      <Button busy={busy} size="sm" type="submit" variant="primary">
        Save
      </Button>
    </form>
  )
}

const HEIGHT = 200
const LEFT = 40
const BOTTOM = 28
const STEP = 24

/** A cycle's burn-up: its scope and its finished tasks, a count a day. */
function BurnUp({ cycle }: { cycle: Cycle }) {
  const days = burnUp(cycle)
  const counted = days.filter((day) => day.scope !== null)
  const ticks = chartTicks(0, Math.max(1, ...counted.map((day) => day.scope ?? 0)))
  const high = ticks.at(-1) ?? 1
  const plot = HEIGHT - BOTTOM - 12
  const y = (value: number) => 12 + plot - (value / (high || 1)) * plot
  const x = (index: number) => LEFT + STEP / 2 + index * STEP
  const width = LEFT + days.length * STEP + 8
  const every = Math.max(1, Math.ceil(days.length / 7))
  const title = `${cycle.title} burn-up`
  return (
    <figure className="aw-cycles__chart">
      <figcaption className="aw-cycles__caption">
        <strong>{title}</strong>
        <ul className="aw-cycles__legend">
          <li>
            <span aria-hidden="true" className="aw-cycles__key aw-cycles__key--scope" />
            Scope
          </li>
          <li>
            <span aria-hidden="true" className="aw-cycles__key aw-cycles__key--done" />
            Done
          </li>
        </ul>
      </figcaption>
      {counted.length === 0 ? (
        <p className="aw-triagerules__note">Counted once a day while the cycle runs.</p>
      ) : (
        <div className="aw-cchart__scroll">
          <svg aria-label={title} className="aw-cchart__line" height={HEIGHT} role="img" viewBox={`0 0 ${width} ${HEIGHT}`} width={width}>
            {ticks.map((tick) => (
              <g key={tick}>
                <line className="aw-cchart__grid" x1={LEFT} x2={width - 8} y1={y(tick)} y2={y(tick)} />
                <text className="aw-cchart__axis" textAnchor="end" x={LEFT - 8} y={y(tick) + 4}>
                  {chartNumber(tick, true)}
                </text>
              </g>
            ))}
            <path className="aw-cycles__line aw-cycles__line--scope" d={linePath(days.map((day) => day.scope), x, y)} />
            <path className="aw-cycles__line aw-cycles__line--done" d={linePath(days.map((day) => day.completed), x, y)} />
            {days.map((day, index) => (
              <g key={day.date}>
                {index % every === 0 ? (
                  <text className="aw-cchart__axis" textAnchor="middle" x={x(index)} y={HEIGHT - 8}>
                    {formatDateLabel(day.date, { month: 'short', day: 'numeric' })}
                  </text>
                ) : null}
                {day.scope === null ? null : (
                  <g>
                    <title>{`${formatDateLabel(day.date)}: ${day.completed ?? 0} of ${day.scope} done`}</title>
                    <rect className="aw-cycles__hit" height={plot} width={STEP} x={x(index) - STEP / 2} y={12} />
                    <circle className="aw-cycles__dot aw-cycles__dot--scope" cx={x(index)} cy={y(day.scope)} r={3} />
                    <circle className="aw-cycles__dot aw-cycles__dot--done" cx={x(index)} cy={y(day.completed ?? 0)} r={3} />
                  </g>
                )}
              </g>
            ))}
          </svg>
        </div>
      )}
      <table className="aw-sr-only">
        <caption>{title}</caption>
        <thead>
          <tr>
            <th scope="col">Day</th>
            <th scope="col">Scope</th>
            <th scope="col">Done</th>
          </tr>
        </thead>
        <tbody>
          {counted.map((day) => (
            <tr key={day.date}>
              <td>{formatDateLabel(day.date)}</td>
              <td>{day.scope}</td>
              <td>{day.completed}</td>
            </tr>
          ))}
        </tbody>
      </table>
    </figure>
  )
}
