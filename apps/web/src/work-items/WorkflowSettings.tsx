import { useState } from 'react'

import { isApiError } from '../api/errors'
import { Banner, Button, MessageSkeleton, Section, SegmentedControl, SelectField, Switch } from '../ui'
import { useWorkItemWorkflow } from './hooks'
import { useSaveWorkflow } from './planningHooks'
import type { EstimateUnit, WorkItemWorkflow } from './types'
import { TransitionMatrix, WorkflowEntries } from './WorkflowEntries'
import {
  addStatus,
  draftFromWorkflow,
  draftProblem,
  moveEntry,
  newRef,
  removeStatus,
  removeType,
  retired,
  setAllowAny,
  togglePair,
  withDefault,
  workflowUpdate,
  type WorkflowDraft,
} from './workflowModel'
import './work.css'

function saveError(error: unknown) {
  if (isApiError(error) && error.code === 'version_conflict') return 'Someone else changed the workflow while you were editing. Discard your changes to see theirs, then make yours again.'
  if (isApiError(error) && error.message) return error.message
  return 'The workflow didn’t save. Check your connection and try again.'
}

/** A Project's statuses, types, allowed status changes and estimate unit. Owners and admins edit it. */
export function WorkflowSettings({ projectId }: { projectId: string }) {
  const workflow = useWorkItemWorkflow(projectId)
  // Lives here so "Workflow saved" survives the editor starting over on the new version.
  const save = useSaveWorkflow()
  if (workflow.isPending) return <MessageSkeleton label="Loading workflow" rows={4} />
  if (workflow.isError) {
    return (
      <Banner action={<Button onClick={() => void workflow.refetch()} size="sm">Try again</Button>} tone="danger">
        The workflow didn’t load.
      </Banner>
    )
  }
  return <WorkflowEditor key={workflow.data.version} onRefresh={() => void workflow.refetch()} projectId={projectId} save={save} saved={workflow.data} />
}

function WorkflowEditor({
  onRefresh,
  projectId,
  save,
  saved,
}: {
  onRefresh: () => void
  projectId: string
  save: ReturnType<typeof useSaveWorkflow>
  saved: WorkItemWorkflow
}) {
  const [draft, setDraft] = useState<WorkflowDraft>(() => draftFromWorkflow(saved))
  const [dirty, setDirty] = useState(false)
  const [tried, setTried] = useState(false)
  const readOnly = !saved.canEdit
  const disabled = readOnly || save.isPending
  const problem = draftProblem(draft, saved)
  const edit = (next: (current: WorkflowDraft) => WorkflowDraft) => {
    setDraft(next)
    setDirty(true)
    save.reset()
  }
  const goneStatuses = retired(saved.statuses, draft.statuses).filter((entry) => entry.taskCount > 0)
  const goneTypes = retired(saved.types, draft.types).filter((entry) => entry.taskCount > 0)

  const submit = () => {
    setTried(true)
    if (problem || save.isPending) return
    save.mutate({ projectId, workflow: workflowUpdate(draft, saved), idempotencyKey: crypto.randomUUID() })
  }

  return (
    <div className="aw-wf">
      {readOnly ? (
        <Banner className="aw-page__banner" icon="lock">
          Only Project owners and admins can change the workflow.
        </Banner>
      ) : null}
      <Section
        actions={
          readOnly ? null : (
            <Button
              disabled={disabled}
              icon="plus"
              onClick={() => edit((current) => addStatus(current, { ref: newRef('status'), name: 'New status', semanticCategory: 'unstarted', isDefault: false, taskCount: 0 }))}
              size="sm"
            >
              Add status
            </Button>
          )
        }
        description="The steps a work item goes through, in board order. What each one means decides what counts as done or in progress."
        id="workflow-statuses-heading"
        title="Statuses"
      >
        <WorkflowEntries
          disabled={disabled}
          entries={draft.statuses}
          noun="status"
          onChange={(ref, change) => edit((current) => ({ ...current, statuses: current.statuses.map((entry) => (entry.ref === ref ? { ...entry, ...change } : entry)) }))}
          onDefault={(ref) => edit((current) => ({ ...current, statuses: withDefault(current.statuses, ref) }))}
          onMove={(index, step) => edit((current) => ({ ...current, statuses: moveEntry(current.statuses, index, step) }))}
          onRemove={(ref) => edit((current) => removeStatus(current, ref))}
        />
      </Section>
      <Section
        actions={
          readOnly ? null : (
            <Button
              disabled={disabled}
              icon="plus"
              onClick={() => edit((current) => ({ ...current, types: [...current.types, { ref: newRef('type'), name: 'New type', isDefault: false, taskCount: 0 }] }))}
              size="sm"
            >
              Add type
            </Button>
          )
        }
        description="Kinds of work, like a bug or a feature."
        id="workflow-types-heading"
        title="Types"
      >
        <WorkflowEntries
          disabled={disabled}
          entries={draft.types}
          noun="type"
          onChange={(ref, change) => edit((current) => ({ ...current, types: current.types.map((entry) => (entry.ref === ref ? { ...entry, ...change } : entry)) }))}
          onDefault={(ref) => edit((current) => ({ ...current, types: withDefault(current.types, ref) }))}
          onMove={(index, step) => edit((current) => ({ ...current, types: moveEntry(current.types, index, step) }))}
          onRemove={(ref) => edit((current) => removeType(current, ref))}
        />
      </Section>
      {goneStatuses.length || goneTypes.length ? (
        <Section description="Work items in what you removed move to what you choose here when you save." id="workflow-moves-heading" title="Where removed work goes">
          <div className="aw-wf__moves">
            {[...goneStatuses.map((entry) => ({ entry, options: draft.statuses })), ...goneTypes.map((entry) => ({ entry, options: draft.types }))].map(({ entry, options }) => (
              <SelectField
                disabled={disabled}
                key={entry.id}
                label={`${entry.taskCount} ${entry.taskCount === 1 ? 'task' : 'tasks'} in ${entry.name} move to`}
                onChange={(event) => edit((current) => ({ ...current, replacements: { ...current.replacements, [entry.id]: event.target.value } }))}
                value={draft.replacements[entry.id] ?? ''}
              >
                <option disabled value="">
                  Choose…
                </option>
                {options.map((option) => (
                  <option key={option.ref} value={option.ref}>
                    {option.name || 'Unnamed'}
                  </option>
                ))}
              </SelectField>
            ))}
          </div>
        </Section>
      ) : null}
      <Section description="Which status changes people and agents can make." id="workflow-transitions-heading" title="Status changes">
        <Switch
          checked={draft.allowAny}
          description="Turn off to choose exactly which changes are allowed."
          disabled={disabled}
          label="Allow any change"
          onChange={(allowAny) => edit((current) => setAllowAny(current, allowAny))}
        />
        {draft.allowAny ? null : (
          <TransitionMatrix
            disabled={disabled}
            onToggle={(from, to) => edit((current) => togglePair(current, from, to))}
            pairs={draft.pairs}
            statuses={draft.statuses}
          />
        )}
      </Section>
      <Section description="How work items are sized." id="workflow-estimate-heading" title="Estimates">
        <SegmentedControl<EstimateUnit>
          label="Estimate unit"
          onChange={(estimateUnit) => {
            if (!disabled) edit((current) => ({ ...current, estimateUnit }))
          }}
          options={[
            { value: 'points', label: 'Points' },
            { value: 'hours', label: 'Hours' },
          ]}
          value={draft.estimateUnit}
        />
      </Section>
      {readOnly ? null : (
        <div className="aw-wf__save" role="group" aria-label="Save workflow">
          {tried && problem ? <Banner tone="warning">{problem}</Banner> : null}
          {save.isError ? <Banner tone="danger">{saveError(save.error)}</Banner> : null}
          {save.isSuccess && !dirty ? <Banner tone="success">Workflow saved.</Banner> : null}
          <div className="aw-wf__savebuttons">
            <Button
              disabled={!dirty || save.isPending}
              onClick={() => {
                setDraft(draftFromWorkflow(saved))
                setDirty(false)
                setTried(false)
                if (isApiError(save.error) && save.error.code === 'version_conflict') onRefresh()
                save.reset()
              }}
              variant="ghost"
            >
              Discard changes
            </Button>
            <Button busy={save.isPending} disabled={!dirty} onClick={submit} variant="primary">
              Save workflow
            </Button>
          </div>
        </div>
      )}
    </div>
  )
}
