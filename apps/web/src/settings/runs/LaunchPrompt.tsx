import { useState, type FormEvent } from 'react'
import { useMutation, useQueryClient } from '@tanstack/react-query'
import { Link } from 'react-router-dom'

import { isApiError } from '../../api/errors'
import { describeError } from '../../app/mutations'
import { Banner, Button, ErrorState, ListSkeleton, TextArea, showToast } from '../../ui'
import {
  resetSettingValue,
  scopeName,
  setSettingValue,
  settingsKeys,
  useEffectiveSettings,
  viewAtScope,
  type EffectiveSettings,
  type SettingsTarget,
} from '../api'
import './runs.css'

const KEY = 'launch.template'

const FIELDS =
  'Fields: {{task.key}}, {{task.title}}, {{task.description}}, {{task.criteria}}, {{task.url}}, {{task.priority}}, {{task.type}}, {{task.labels}}, {{task.pages}}, {{project.name}}, {{project.key}}, {{project.guide}}, {{project.skills}}. Wrap a part in {{#if field}}…{{/if}} to keep it only when the field has a value, or in {{#harness id}}…{{/harness}} to keep it for one harness.'

/** The prompt Launch writes from a task, for the whole workspace or one project, with the fields it can use. */
export function LaunchPrompt({ target, workspaceHref }: { target: SettingsTarget; workspaceHref?: string }) {
  const queryClient = useQueryClient()
  const effective = useEffectiveSettings(target)
  const [draft, setDraft] = useState<string | null>(null)
  const [notice, setNotice] = useState<{ tone: 'warning' | 'danger'; text: string } | null>(null)
  const resolved = effective.data?.settings.find((setting) => setting.key === KEY)
  const view = resolved ? viewAtScope(resolved, target.scope) : null
  const save = useMutation({
    mutationFn: (value: string | null) => {
      const expectedVersion = view?.own?.version ?? 0
      return value === null ? resetSettingValue(target, KEY, expectedVersion) : setSettingValue(target, KEY, value, expectedVersion)
    },
    onMutate: () => setNotice(null),
    onSuccess: ({ setting }, value) => {
      queryClient.setQueryData<EffectiveSettings>(settingsKeys.effective(target), (current) =>
        current ? { ...current, settings: current.settings.map((candidate) => (candidate.key === KEY ? setting : candidate)) } : current,
      )
      // The launch dialog reads the same settings under its own key.
      if (target.scope === 'project') void queryClient.invalidateQueries({ queryKey: ['project', target.projectId, 'settings'] })
      setDraft(null)
      if (value !== null) showToast({ tone: 'success', title: 'Launch prompt saved. The next launch uses it.' })
    },
    onError: async (error) => {
      if (isApiError(error) && (error.code === 'version_conflict' || error.status === 409)) {
        await queryClient.invalidateQueries({ queryKey: settingsKeys.effective(target) })
        setNotice({ tone: 'warning', text: 'Someone else changed the prompt just now. Yours is still in the box; save again to replace theirs.' })
      } else {
        setNotice({ tone: 'danger', text: describeError(error).message })
      }
    },
  })

  if (effective.isPending) return <ListSkeleton label="Loading the launch prompt" rows={3} />
  if (effective.isError || !view) {
    return (
      <ErrorState
        action={
          <Button onClick={() => void effective.refetch()} size="sm">
            Try again
          </Button>
        }
        title="The launch prompt didn’t load"
      >
        Check your connection and try again.
      </ErrorState>
    )
  }
  const canEdit = target.scope === 'project' ? effective.data.capabilities.manageProject : effective.data.capabilities.manageWorkspace
  const current = typeof view.value === 'string' ? view.value : ''
  const text = draft ?? current
  const changed = draft !== null && draft !== current
  const source = view.own ? target.scope : view.inherited.source
  const fallback = view.inherited.source === 'default' ? 'the built-in prompt' : 'the workspace’s prompt'

  const submit = (event: FormEvent<HTMLFormElement>) => {
    event.preventDefault()
    if (changed) save.mutate(draft)
  }

  return (
    <form className="aw-runs__prompt" id={`setting-${KEY}`} onSubmit={submit} tabIndex={-1}>
      <p className="aw-setting__source">
        {source === 'default' ? (
          'The built-in prompt.'
        ) : (
          <>
            Set in:{' '}
            {source === 'workspace' && target.scope === 'project' && workspaceHref ? (
              <Link to={`${workspaceHref}#launch-prompt`}>{scopeName(source)}</Link>
            ) : (
              scopeName(source)
            )}
            .
          </>
        )}
      </p>
      <TextArea
        disabled={!canEdit}
        hint={FIELDS}
        id={`launch-template-${target.scope}`}
        label="Prompt template"
        maxLength={12000}
        onChange={(event) => setDraft(event.target.value)}
        rows={14}
        spellCheck={false}
        value={text}
      />
      {notice ? <Banner tone={notice.tone}>{notice.text}</Banner> : null}
      {canEdit ? (
        <div className="aw-runs__actions">
          <Button busy={save.isPending && save.variables !== null} disabled={!changed} type="submit" variant="primary">
            Save prompt
          </Button>
          {changed ? (
            <Button onClick={() => setDraft(null)} variant="ghost">
              Discard changes
            </Button>
          ) : null}
          {view.own && !changed ? (
            <Button busy={save.isPending && save.variables === null} icon="undo" onClick={() => save.mutate(null)} variant="ghost">
              Use {fallback}
            </Button>
          ) : null}
        </div>
      ) : null}
    </form>
  )
}
