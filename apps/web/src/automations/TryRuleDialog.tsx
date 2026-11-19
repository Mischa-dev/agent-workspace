import { useState } from 'react'
import { useMutation, useQuery } from '@tanstack/react-query'

import { describeError } from '../app/mutations'
import { Banner, Button, Dialog, ErrorState, ListSkeleton, Row, RowList, showToast } from '../ui'
import { TaskSearchPicker } from '../work-items/TaskSearchPicker'
import { automationKeys, previewRule, runRule, type AutomationRule, type RulePreview } from './api'
import { AUTOMATION_OPERATIONS, fieldName } from './operationForm'

/** A step as people read it: an operation by its title, not its name. */
function stepTitle(step: RulePreview['actions'][number]) {
  if (step.kind !== 'run_operation') return step.summary
  const name = (step.action as { operation?: unknown }).operation
  const entry = AUTOMATION_OPERATIONS.find((candidate) => candidate.name === name)
  return entry ? `Run “${entry.title}”` : step.summary
}

/**
 * Try a rule (W1.42): what it would do if it fired now, optionally on one
 * task, worked out by the server without sending, changing or recording
 * anything. Running it for real is a separate, deliberate step.
 */
export function TryRuleDialog({
  onClose,
  onRan,
  projectId,
  rule,
}: {
  onClose: () => void
  onRan: () => void
  projectId: string
  rule: AutomationRule
}) {
  const [task, setTask] = useState<{ id: string; key: string } | null>(null)
  const [picking, setPicking] = useState<HTMLElement | null>(null)
  const preview = useQuery({
    queryKey: automationKeys.preview(projectId, rule.id, task?.id ?? null),
    queryFn: () => previewRule(projectId, rule.id, task?.id),
    staleTime: 0,
    gcTime: 0,
  })
  const run = useMutation({
    mutationFn: () => runRule(projectId, rule.id, task?.id),
    onSuccess: (result) => {
      showToast({
        tone: result.state === 'failed' ? 'danger' : 'success',
        title: result.state === 'pending' ? 'Running it now' : 'It ran',
        description: result.detail ?? undefined,
      })
      onRan()
    },
  })
  const nothing = preview.data?.skipped ?? null

  return (
    <Dialog
      footer={
        <>
          <Button onClick={onClose}>Done</Button>
          <Button busy={run.isPending} disabled={!preview.data || Boolean(nothing)} icon="play" onClick={() => run.mutate()} variant="primary">
            Run it for real
          </Button>
        </>
      }
      onClose={onClose}
      open
      title={`Try ${rule.name}`}
      width={560}
    >
      <div className="aw-rule-try" data-part="automation-try">
        <p className="aw-quiet">This shows what it would do now. Nothing is sent or changed until you run it.</p>
        <div className="aw-rule-try__task">
          <span>{task ? `As if it fired for ${task.key}` : 'As if it fired now'}</span>
          <Button icon="search" onClick={(event) => setPicking(event.currentTarget)} size="sm">
            {task ? 'Another task' : 'Choose a task'}
          </Button>
          {task ? (
            <Button onClick={() => setTask(null)} size="sm" variant="ghost">
              No task
            </Button>
          ) : null}
        </div>
        {picking ? (
          <TaskSearchPicker
            allowTypedKey={false}
            anchor={picking}
            exclude={[]}
            label="Try it on"
            onClose={() => setPicking(null)}
            onPick={(picked) => {
              setPicking(null)
              if (picked.id) setTask({ id: picked.id, key: picked.key })
            }}
            projectId={projectId}
          />
        ) : null}
        {preview.isPending ? (
          <ListSkeleton label="Working out what it would do" rows={2} />
        ) : preview.isError ? (
          <ErrorState error={preview.error} onRetry={() => void preview.refetch()} title="It couldn’t be tried" />
        ) : nothing ? (
          <Banner tone="warning">Nothing would happen. {nothing}</Banner>
        ) : (
          <RowList label="What it would do">
            {preview.data.actions.map((step, index) => (
              <Row
                key={index}
                leading={
                  <span aria-hidden="true" className="aw-row__tile">
                    {index + 1}
                  </span>
                }
                meta={step.kind === 'run_operation' ? <OperationInput input={(step.action as { input?: unknown }).input} /> : undefined}
                title={stepTitle(step)}
              />
            ))}
          </RowList>
        )}
        {!task && preview.data && !nothing ? (
          <p className="aw-quiet">Task details like {'{{task.title}}'} stay empty until you choose a task.</p>
        ) : null}
        {run.isError ? <Banner tone="danger">It didn’t run. {describeError(run.error).message}</Banner> : null}
      </div>
    </Dialog>
  )
}

/** An operation's input as it would be sent, a line per answer, the space it runs in left out. */
function OperationInput({ input }: { input: unknown }) {
  const entries = Object.entries(input && typeof input === 'object' ? (input as Record<string, unknown>) : {}).filter(
    ([key]) => key !== 'projectId' && key !== 'spaceId',
  )
  if (!entries.length) return null
  return (
    <dl className="aw-rule-try__input">
      {entries.map(([key, value]) => (
        <div key={key}>
          <dt>{fieldName(key)}</dt>
          <dd>{typeof value !== 'string' ? JSON.stringify(value) : value || <span className="aw-quiet">(empty)</span>}</dd>
        </div>
      ))}
    </dl>
  )
}
