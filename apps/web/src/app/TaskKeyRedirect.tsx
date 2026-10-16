import { useState } from 'react'
import { useQuery } from '@tanstack/react-query'
import { Navigate, useNavigate, useParams } from 'react-router-dom'

import { useAuthenticatedSession } from '../auth/session'
import { AppShell } from '../shell/AppShell'
import { parseTaskKey } from '../shell/shortcuts'
import { useProjectByKey } from '../shell/useProjectByKey'
import { Button, EmptyState, ErrorState, PageSkeleton } from '../ui'
import { searchWorkItemReferences } from '../work-items/api'

/** `/AWD-12` opens task 12 of the Project whose key is AWD. */
export function TaskKeyRedirect() {
  const session = useAuthenticatedSession()
  const navigate = useNavigate()
  const { taskKey = '' } = useParams()
  const parsed = parseTaskKey(taskKey)
  // Keys repeat across organizations: when they do, the person picks one.
  const [chosenId, setChosenId] = useState<string | null>(null)
  const byKey = useProjectByKey(parsed?.projectKey ?? null, chosenId)
  const { project } = byKey
  const lookup = useQuery({
    queryKey: ['project', project?.id ?? '', 'task-key', parsed?.key ?? ''],
    queryFn: async () => {
      if (!project || !parsed) return null
      const matches = await searchWorkItemReferences(project.id, parsed.key)
      return matches.find((match) => match.key.toUpperCase() === parsed.key) ?? null
    },
    enabled: Boolean(project && parsed),
    retry: false,
  })

  if (lookup.data) return <Navigate replace to={lookup.data.href} />

  const loading = Boolean(parsed) && (byKey.pending || (Boolean(project) && lookup.isPending))
  const failed = Boolean(parsed) && !loading && Boolean(byKey.error || lookup.isError)
  const choosing = Boolean(parsed) && !loading && !failed && !project && byKey.matches.length > 1
  return (
    <AppShell mainClassName="aw-main--page" project={project} user={session.user}>
      <div className="aw-page" role={loading || failed ? undefined : 'alert'}>
        {loading ? (
          <PageSkeleton label={`Opening ${parsed?.key ?? 'task'}`} />
        ) : failed ? (
          <ErrorState
            action={
              <Button
                onClick={() => {
                  byKey.retry()
                  if (project) void lookup.refetch()
                }}
                variant="primary"
              >
                Try again
              </Button>
            }
            title={`Couldn’t open ${parsed?.key}`}
          >
            Check your connection and try again.
          </ErrorState>
        ) : choosing ? (
          <EmptyState icon="search" title={`Which ${parsed?.projectKey} Project?`}>
            <p>More than one Project you can see uses this key.</p>
            <div className="aw-stack">
              {byKey.matches.map((match) => (
                <Button key={match.id} onClick={() => setChosenId(match.id)}>
                  {`Open ${parsed?.key} in ${match.name}`}
                </Button>
              ))}
            </div>
          </EmptyState>
        ) : (
          <EmptyState
            action={
              <Button onClick={() => navigate(project ? `/projects/${encodeURIComponent(project.id)}/work-items` : '/home')} variant="primary">
                {project ? `Go to ${project.name} tasks` : 'Go to Home'}
              </Button>
            }
            icon="search"
            title={parsed ? `${parsed.key} wasn’t found` : 'Page not found'}
          >
            {parsed
              ? project
                ? 'That task may have been removed, or you may not have access to it.'
                : 'No Project you can see uses that key.'
              : 'That address isn’t part of this workspace. It may have moved, or the link may be old.'}
          </EmptyState>
        )}
      </div>
    </AppShell>
  )
}
