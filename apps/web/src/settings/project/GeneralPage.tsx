import { useEffect, useRef, useState, type FormEvent } from 'react'
import { useNavigate } from 'react-router-dom'

import { isApiError } from '../../api/errors'
import { roleLabel } from '../../projects/format'
import { useArchiveProject, useProject } from '../../projects/hooks'
import { ProjectInitials } from '../../projects/ProjectInitials'
import { Banner, Button, DetailList, PageHeader, Section, TextField } from '../../ui'
import { markArchiving } from '../archiving'
import type { SettingsPageProps } from '../types'

/** Project › General: about the Project, and archiving it. */
export default function GeneralPage({ project: given, workspace }: SettingsPageProps) {
  const projectId = given?.id ?? ''
  const project = useProject(projectId)
  const archive = useArchiveProject()
  const navigate = useNavigate()
  const errorRef = useRef<HTMLDivElement>(null)
  const [confirmation, setConfirmation] = useState('')
  const [error, setError] = useState<string | null>(null)
  useEffect(() => {
    if (error) errorRef.current?.focus()
  }, [error])
  const current = project.data ?? given
  if (!current) return null

  const submitArchive = async (event: FormEvent<HTMLFormElement>) => {
    event.preventDefault()
    if (confirmation !== current.key) return
    setError(null)
    markArchiving(current.id, true)
    try {
      await archive.mutateAsync({ projectId: current.id, expectedVersion: current.version })
      navigate('/home', {
        replace: true,
        state: { notice: `${current.name} was archived. Everything in it was kept.` },
      })
    } catch (reason) {
      // Only a failure clears it: after success the page leaves for Home.
      markArchiving(current.id, false)
      if (isApiError(reason) && reason.code === 'version_conflict') {
        await project.refetch()
        setError('Someone changed this Project while you were here. Check the details above and try again.')
      } else {
        setError(isApiError(reason) ? reason.message : 'The Project wasn’t archived. Try again.')
      }
    }
  }

  return (
    <>
      <PageHeader mark={<ProjectInitials project={current} size={40} />} title="General">
        {current.name} · {current.key}
      </PageHeader>
      <Section id="project-details-heading" title="About this Project">
        <DetailList
          items={[
            { label: 'Name', value: current.name },
            { label: 'Key', value: <code className="aw-inline-code">{current.key}</code> },
            { label: 'Your role', value: roleLabel(current.role) },
            ...(workspace ? [{ label: 'Workspace', value: workspace.name }] : []),
          ]}
        />
        {current.role === 'viewer' && (
          <Banner role="note" tone="info">
            Viewer access is read-only. You can look at the Project and its repositories, but members, agents, runs and
            GitHub are managed by members and admins.
          </Banner>
        )}
      </Section>
      {current.capabilities.archive && !current.archivedAt && (
        <Section
          description="Archiving takes it out of everyone’s sidebar and stops new messages, work items and runs. Everything already in it is kept and stays readable."
          id="archive-project-heading"
          title="Archive this Project"
        >
          <form className="aw-danger" onSubmit={submitArchive}>
            {error && (
              <Banner ref={errorRef} tabIndex={-1} tone="danger">
                {error}
              </Banner>
            )}
            <TextField
              autoComplete="off"
              id="archive-confirmation"
              label={
                <>
                  Type <strong>{current.key}</strong> to confirm
                </>
              }
              mono
              onChange={(event) => setConfirmation(event.target.value)}
              spellCheck={false}
              value={confirmation}
            />
            <div>
              <Button busy={archive.isPending} disabled={confirmation !== current.key} icon="archive" type="submit" variant="danger">
                Archive Project
              </Button>
            </div>
          </form>
        </Section>
      )}
    </>
  )
}
