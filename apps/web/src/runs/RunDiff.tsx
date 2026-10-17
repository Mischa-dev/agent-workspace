import { useQuery } from '@tanstack/react-query'
import { useState } from 'react'

import { Banner, Button, ListSkeleton } from '../ui'
import { getRunDiff, runKeys, type ChangedFile } from './api'
import { splitPatch, type PatchFile } from './runModel'

const STATUS_LABEL: Record<string, string> = { added: 'Added', modified: 'Changed', deleted: 'Deleted', renamed: 'Renamed' }

/** The files the run changed, with the diff fetched only when asked for. */
export function RunDiff({ files, runId }: { files: ChangedFile[] | null; runId: string }) {
  const [wanted, setWanted] = useState(false)
  const diff = useQuery({ queryKey: runKeys.diff(runId), queryFn: () => getRunDiff(runId), enabled: wanted })

  if (files === null) {
    return <p className="aw-runs__quiet">The run’s branch is gone, so its changes can’t be read any more.</p>
  }
  if (files.length === 0) return <p className="aw-runs__quiet">The run hasn’t changed any files.</p>

  const patches = diff.data?.patch ? splitPatch(diff.data.patch) : []
  const tooBig = diff.data?.state === 'ready' && (diff.data.truncated || !diff.data.patch)

  return (
    <>
      <ul className="aw-rundiff__files">
        {files.map((file) => (
          <li className="aw-rundiff__file" key={file.path}>
            <code className="aw-rundiff__path">{file.path}</code>
            <span className="aw-rundiff__stat">
              {STATUS_LABEL[file.status] ?? file.status}
              {file.added === null ? ' · binary' : (
                <>
                  {' · '}
                  <span className="aw-rundiff__added">+{file.added}</span> <span className="aw-rundiff__removed">−{file.removed ?? 0}</span>
                </>
              )}
            </span>
          </li>
        ))}
      </ul>
      {!wanted ? (
        <Button icon="branch" onClick={() => setWanted(true)}>
          Show the diff
        </Button>
      ) : null}
      {wanted && diff.isPending ? <ListSkeleton label="Loading the diff" rows={3} /> : null}
      {diff.isError ? (
        <Banner action={<Button onClick={() => void diff.refetch()}>Try again</Button>} tone="danger" title="The diff didn’t load.">
          {diff.error.message}
        </Banner>
      ) : null}
      {diff.data?.state === 'unavailable' ? (
        <Banner tone="info">The diff can’t be read now: the run’s worktree and published commit are both gone.</Banner>
      ) : null}
      {tooBig ? (
        <Banner tone="warning" title="The diff is too big to show here.">
          {patches.length > 0 ? 'Only the first part is shown. ' : null}Open the branch or pull request to read all of it.
        </Banner>
      ) : null}
      {patches.length > 0 ? (
        <div className="aw-rundiff__patches">
          {patches.map((patch) => (
            <PatchView key={patch.path} patch={patch} />
          ))}
        </div>
      ) : null}
    </>
  )
}

function lineClass(line: string) {
  if (line.startsWith('+++') || line.startsWith('---') || line.startsWith('diff ') || line.startsWith('index ')) return 'aw-rundiff__line--meta'
  if (line.startsWith('@@')) return 'aw-rundiff__line--hunk'
  if (line.startsWith('+')) return 'aw-rundiff__line--add'
  if (line.startsWith('-')) return 'aw-rundiff__line--del'
  return undefined
}

function PatchView({ patch }: { patch: PatchFile }) {
  const [open, setOpen] = useState(true)
  return (
    <div className="aw-rundiff__patch">
      <Button aria-expanded={open} icon={open ? 'chevron-down' : 'chevron-right'} onClick={() => setOpen(!open)} size="sm" variant="ghost">
        <code>{patch.path}</code>
      </Button>
      {open ? (
        <pre className="aw-runs__pre aw-rundiff__pre">
          {patch.text.split('\n').map((line, index) => (
            <span className={lineClass(line)} key={index}>
              {line}
              {'\n'}
            </span>
          ))}
        </pre>
      ) : null}
    </div>
  )
}
