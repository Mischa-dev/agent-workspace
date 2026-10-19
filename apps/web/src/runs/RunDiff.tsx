import { useQuery, useQueryClient } from '@tanstack/react-query'
import { useEffect, useRef, useState } from 'react'

import { Banner, Button, ListSkeleton, describeError } from '../ui'
import { getRunDiff, runKeys, type RunDetail } from './api'
import { splitPatch, type PatchFile } from './runModel'

const STATUS_LABEL: Record<string, string> = { added: 'Added', modified: 'Changed', deleted: 'Deleted', renamed: 'Renamed' }

/** Why a run has no files to list: not started, no branch, or its branch is gone. */
function noFilesText(run: Pick<RunDetail, 'agentName' | 'baseCommit' | 'publishedCommit'>, live: boolean) {
  if (live) return `Changes show up here once ${run.agentName ?? 'the agent'} edits files.`
  if (!run.baseCommit) return 'This run didn’t work on a branch, so it has no changes to show.'
  if (!run.publishedCommit) return 'The run’s branch is gone and was never published, so its changes can’t be read any more.'
  return 'The run’s changes can’t be read right now.'
}

/** The files the run changed, with the diff fetched only when asked for. */
export function RunDiff({ live, run }: { live: boolean; run: Pick<RunDetail, 'agentName' | 'baseCommit' | 'files' | 'id' | 'publishedCommit'> }) {
  const { files } = run
  const [wanted, setWanted] = useState(false)
  const queryClient = useQueryClient()
  const diff = useQuery({ queryKey: runKeys.diff(run.id), queryFn: () => getRunDiff(run.id), enabled: wanted })

  // A run that keeps editing files gets a fresh diff, not the one from when it was opened.
  const signature = files?.map((file) => `${file.status}:${file.path}:${file.added ?? ''}:${file.removed ?? ''}`).join('|') ?? ''
  const seen = useRef(signature)
  useEffect(() => {
    if (seen.current === signature) return
    seen.current = signature
    if (wanted) void queryClient.invalidateQueries({ queryKey: runKeys.diff(run.id) })
  }, [queryClient, run.id, signature, wanted])

  if (files === null) return <p className="aw-runs__quiet">{noFilesText(run, live)}</p>
  if (files.length === 0) {
    return <p className="aw-runs__quiet">{live ? `No files changed yet. They show up here as ${run.agentName ?? 'the agent'} edits them.` : 'The run didn’t change any files.'}</p>
  }

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
              {file.added === null ? (
                ' · binary'
              ) : (
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
        <Banner onRetry={() => void diff.refetch()} tone="danger" title="The diff didn’t load.">
          {describeError(diff.error).message}
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
      <Button
        aria-expanded={open}
        className="aw-rundiff__toggle"
        icon={open ? 'chevron-down' : 'chevron-right'}
        onClick={() => setOpen(!open)}
        size="sm"
        variant="ghost"
      >
        <code>{patch.path}</code>
      </Button>
      {open ? (
        <pre className="aw-runs__pre aw-rundiff__pre">
          {/* One block per line, inside a box as wide as the longest, so a line's colour runs the full width when scrolled. */}
          <span className="aw-rundiff__lines">
            {patch.text.split('\n').map((line, index) => (
              <span className={lineClass(line)} key={index}>
                {line || ' '}
              </span>
            ))}
          </span>
        </pre>
      ) : null}
    </div>
  )
}
