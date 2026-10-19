import { useMutation, useQuery, useQueryClient } from '@tanstack/react-query'
import { Link } from 'react-router-dom'

import { isApiError } from '../../api/errors'
import { queryKeys } from '../../api/queryKeys'
import { describeError } from '../../app/mutations'
import {
  listHarnesses,
  listWorkspaceHarnesses,
  pickableModels,
  refreshCatalog,
  saveModel,
  type Harness,
  type ModelOverrides,
} from '../../execution/api'
import { Banner, Button, EmptyState, ErrorState, ListSkeleton, Row, RowList, StatusChip, formatShortDate, showToast } from '../../ui'
import { accountSettingsHref, workspaceSettingsHref } from '../paths'

/**
 * The workspace's model lists, one per harness. Owners and admins hide models
 * nobody should pick, choose each harness's default and fetch the provider's
 * latest list; everyone else sees what they can pick.
 */
export function WorkspaceModelLists({ organizationId }: { organizationId: string }) {
  const harnesses = useQuery({
    queryKey: queryKeys.modelCatalog(organizationId),
    queryFn: () => listWorkspaceHarnesses(organizationId),
    enabled: Boolean(organizationId),
  })
  if (harnesses.isPending) return <ListSkeleton label="Loading the model lists" rows={3} />
  if (harnesses.isError) {
    return (
      <ErrorState
        action={
          <Button onClick={() => void harnesses.refetch()} size="sm">
            Try again
          </Button>
        }
        title="The model lists didn’t load"
      >
        Check your connection and try again.
      </ErrorState>
    )
  }
  if (!harnesses.data.length) {
    return (
      <EmptyState
        action={
          <Link className="aw-btn aw-btn--secondary aw-btn--md" to={workspaceSettingsHref('harnesses')}>
            <span className="aw-btn__label">Add a harness</span>
          </Link>
        }
        icon="terminal"
        title="No harnesses on this server"
      >
        Models appear here once the server has a harness to run.
      </EmptyState>
    )
  }
  return (
    <div className="aw-settings-stack">
      {harnesses.data.map((harness) => (
        <ModelList harness={harness} key={harness.kind} organizationId={organizationId} />
      ))}
    </div>
  )
}

function ModelList({ harness, organizationId }: { harness: Harness; organizationId: string }) {
  const queryClient = useQueryClient()
  const editable = harness.canEditCatalog
  const saved = (updated: Harness) =>
    queryClient.setQueryData<Harness[]>(queryKeys.modelCatalog(organizationId), (current) =>
      current?.map((entry) => (entry.kind === updated.kind ? updated : entry)),
    )
  const edit = useMutation({
    mutationFn: ({ modelId, change }: { modelId: string; change: ModelOverrides }) =>
      saveModel(organizationId, harness.kind, modelId, change, crypto.randomUUID()),
    onSuccess: saved,
  })
  const refresh = useMutation({
    mutationFn: () => refreshCatalog(organizationId, harness.kind, crypto.randomUUID()),
    onSuccess: (updated) => {
      saved(updated)
      showToast({ tone: 'success', title: `${harness.displayName}’s list is up to date` })
    },
  })
  // People who can't change the list only need what they can pick.
  const models = editable ? harness.catalog.models : pickableModels(harness)
  const busyModel = edit.isPending ? edit.variables?.modelId : undefined
  const counted = pickableModels(harness).length

  return (
    <section aria-labelledby={`models-${harness.kind}`} className="aw-card aw-profile">
      <header className="aw-profile__head">
        <div className="aw-profile__titles">
          <h3 className="aw-profile__name" id={`models-${harness.kind}`}>
            {harness.displayName}
          </h3>
          <p className="aw-profile__sub">
            {counted === 1 ? '1 model to pick from' : counted ? `${counted} models to pick from` : 'No models to pick from'}
            {harness.catalog.refreshedAt ? ` · Updated ${formatShortDate(harness.catalog.refreshedAt)}` : ''}
          </p>
        </div>
        {editable && harness.features.modelListing ? (
          <Button busy={refresh.isPending} icon="retry" onClick={() => refresh.mutate()} size="sm">
            Refresh list
          </Button>
        ) : null}
      </header>
      {refresh.isError ? <RefreshFailure error={refresh.error} harness={harness} /> : null}
      {edit.isError ? <Banner tone="danger">{describeError(edit.error).message}</Banner> : null}
      {models.length ? (
        <RowList label={`${harness.displayName} models`}>
          {models.map((model) => (
            <Row
              actions={
                editable ? (
                  <>
                    {!model.isDefault && !model.hidden ? (
                      <Button
                        aria-label={`Make ${model.displayName} the default`}
                        disabled={busyModel === model.id}
                        onClick={() => edit.mutate({ modelId: model.id, change: { ...model.overrides, isDefault: true } })}
                        size="sm"
                        variant="ghost"
                      >
                        Make default
                      </Button>
                    ) : null}
                    {!model.isDefault || model.hidden ? (
                      <Button
                        aria-label={`${model.hidden ? 'Show' : 'Hide'} ${model.displayName}`}
                        disabled={busyModel === model.id}
                        onClick={() => edit.mutate({ modelId: model.id, change: { ...model.overrides, hidden: !model.hidden } })}
                        size="sm"
                        variant="ghost"
                      >
                        {model.hidden ? 'Show' : 'Hide'}
                      </Button>
                    ) : null}
                  </>
                ) : undefined
              }
              key={model.id}
              meta={
                <span className="aw-row__metaline aw-wrap">
                  <span>{model.id}</span>
                  {model.source === 'manual' ? <span>Added by hand</span> : null}
                </span>
              }
              title={
                <span className="aw-row__titleline">
                  {model.displayName}
                  {model.isDefault ? <StatusChip tone="success">Default</StatusChip> : null}
                  {model.hidden ? <StatusChip>Hidden</StatusChip> : null}
                </span>
              }
            />
          ))}
        </RowList>
      ) : (
        <p className="aw-quiet">
          {!harness.features.modelListing
            ? 'This harness doesn’t list its models, so runs use its own default.'
            : editable
              ? 'Nothing listed yet. Refresh the list to fetch it with your signed-in account.'
              : 'Nothing listed yet. An owner or admin can fetch the list from the provider.'}
        </p>
      )}
    </section>
  )
}

function RefreshFailure({ error, harness }: { error: unknown; harness: Harness }) {
  if (isApiError(error) && error.code === 'execution_profile_required') {
    return (
      <Banner tone="warning">
        The list comes through your own {harness.displayName} account. Sign in to one in{' '}
        <Link to={accountSettingsHref('provider-accounts')}>Provider accounts</Link> first.
      </Banner>
    )
  }
  if (isApiError(error) && error.status === 409) {
    return <Banner tone="danger">{harness.displayName} couldn’t list its models just now. Try again in a moment.</Banner>
  }
  return <Banner tone="danger">{describeError(error).message}</Banner>
}

/**
 * What people in a project can pick: the workspace's lists, one line per
 * harness. The lists themselves are changed in the workspace's settings.
 */
export function ProjectModelLists({ projectId }: { projectId: string }) {
  const harnesses = useQuery({ queryKey: queryKeys.harnesses(projectId), queryFn: () => listHarnesses(projectId) })
  if (harnesses.isPending) return <ListSkeleton label="Loading the model lists" rows={2} />
  if (harnesses.isError) {
    return (
      <ErrorState
        action={
          <Button onClick={() => void harnesses.refetch()} size="sm">
            Try again
          </Button>
        }
        title="The model lists didn’t load"
      >
        Check your connection and try again.
      </ErrorState>
    )
  }
  if (!harnesses.data.length) {
    return (
      <EmptyState icon="terminal" title="No harnesses on this server">
        Models appear here once the server has a harness to run.
      </EmptyState>
    )
  }
  return (
    <RowList label="Models by harness">
      {harnesses.data.map((harness) => {
        const models = pickableModels(harness)
        return (
          <Row
            key={harness.kind}
            meta={
              models.length
                ? models.map((model) => (model.isDefault ? `${model.displayName} (default)` : model.displayName)).join(', ')
                : 'Uses its own default model'
            }
            title={harness.displayName}
          />
        )
      })}
    </RowList>
  )
}
