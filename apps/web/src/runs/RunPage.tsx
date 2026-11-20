import { useParams } from 'react-router-dom'

import { here } from '../app/href'
import { useShellPage } from '../shell/shellStore'
import { ButtonLink } from '../ui'
import { RunView } from './RunView'
import './runs.css'

/** `/w/:ws/runs/:runId`: one run's page, drawn by the shared run view. */
export function RunPage() {
  const { runId = '' } = useParams()
  useShellPage({ home: true, mainClassName: 'aw-main--page' })
  return (
    <div className="aw-page aw-runs">
      <ButtonLink className="aw-runs__back" icon="arrow-left" size="sm" to={here('/runs')} variant="ghost">
        All runs
      </ButtonLink>
      <RunView runId={runId} />
    </div>
  )
}
