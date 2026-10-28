import { NavigationSyncAlert } from './NavigationSyncAlert'
import type { Project, ProjectLocation } from './types'
import { useRememberProjectLocation } from './useRememberProjectLocation'

/**
 * Remembers this page as the person's place in the project, so the project
 * opens here next time. Says so only if that couldn't be saved.
 */
export function RememberPlace({ location, project }: { location: ProjectLocation; project: Project }) {
  const sync = useRememberProjectLocation(
    project.id,
    location,
    !project.archivedAt && project.capabilities.navigationWrite,
    project.navigation,
  )
  return <NavigationSyncAlert {...sync} />
}
