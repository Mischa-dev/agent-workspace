import { FieldManager } from '../../fields/FieldManager'
import { PageHeader } from '../../ui'
import type { SettingsPageProps } from '../types'

/** Project › Fields: the custom fields every task in the project has (AWD-89). */
export default function FieldsPage({ project }: SettingsPageProps) {
  if (!project) return null
  return (
    <>
      <PageHeader icon="list" title="Fields">
        Fields every task in {project.name} has, such as Risk or Customer. They show on each task, as list columns and on board
        cards, and saved views filter, sort and group by them.
      </PageHeader>
      <FieldManager scope={{ projectId: project.id }} />
    </>
  )
}
