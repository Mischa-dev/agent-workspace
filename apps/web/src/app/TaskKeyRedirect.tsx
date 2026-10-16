import { useQuery } from '@tanstack/react-query'
import { Navigate, useNavigate, useParams } from 'react-router-dom'

import { useAuthenticatedSession } from '../auth/session'
import { AppShell } from '../shell/AppShell'
import { parseTaskKey } from '../shell/shortcuts'
import { useProjectByKey } from '../shell/useProjectByKey'
import { Button, EmptyState, PageSkeleton } from '../ui'
import { searchWorkItemReferences } from '../work-items/api'

/** `/AWD-12` opens task 12 of the Project whose key is AWD. */
export function TaskKeyRedirect() {
  const session = useAuthenticatedSession()
  const navigate = useNavigate()
  const { taskKey = '' } = useParams()
  const parsed = parseTaskKey(taskKey)
  const { project, pending: projectPending } = useProjectByKey(parsed?.projectKey ?? null)
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

  const loading = Boolean(parsed) && (projectPending || (Boolean(project) && lookup.isPending))
  return (
    <AppShell mainClassName="aw-main--page" project={project} user={session.user}>
      <div className="aw-page" role={loading ? undefined : 'alert'}>
        {loading ? (
          <PageSkeleton label={`Opening ${parsed?.key ?? 'task'}`} />
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
