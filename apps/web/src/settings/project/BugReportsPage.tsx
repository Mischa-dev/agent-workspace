import { useEffect, useRef, useState, type FormEvent } from 'react'
import { useMutation, useQuery, useQueryClient } from '@tanstack/react-query'
import { Bug } from 'lucide-react'

import { describeError, toastError } from '../../app/mutations'
import {
  Banner,
  Button,
  ConfirmDialog,
  Dialog,
  EmptyState,
  ErrorState,
  Icon,
  IconButton,
  ListSkeleton,
  Menu,
  PageHeader,
  Row,
  RowList,
  Section,
  SegmentedControl,
  StatusChip,
  TextField,
  countLabel,
  formatFullDate,
  formatRelative,
  showToast,
  type ChipTone,
} from '../../ui'
import type { Project } from '../../projects/types'
import type { SettingsPageProps } from '../types'
import {
  SNIPPET_KINDS,
  bugReportKeys,
  createBugReportLink,
  deleteBugReportLink,
  listBugReportLinks,
  replaceBugReportLink,
  snippet,
  updateBugReportLink,
  type BugReportLink,
  type SnippetKind,
} from './bugReports'
import '../../styles/pages.css'

const STATE: Record<BugReportLink['state'], { label: string; tone: ChipTone }> = {
  active: { label: 'On', tone: 'success' },
  paused: { label: 'Paused', tone: 'muted' },
  disabled: { label: 'Turned off', tone: 'danger' },
}

/** Project › Bug reports: links other apps send bug reports to, each becoming a task here. */
export default function BugReportsPage({ project }: SettingsPageProps) {
  // Keyed by project, so switching projects drops a shown link and any open
  // dialog instead of showing one project's secret under another's name.
  return project ? <BugReports key={project.id} project={project} /> : null
}

function BugReports({ project }: { project: Project }) {
  const projectId = project.id
  const queryClient = useQueryClient()
  const list = useQuery({
    queryKey: bugReportKeys.links(projectId),
    queryFn: () => listBugReportLinks(projectId),
  })
  const [creating, setCreating] = useState(false)
  const [setup, setSetup] = useState<{ name: string; url: string } | null>(null)
  const [replacing, setReplacing] = useState<BugReportLink | null>(null)
  const [deleting, setDeleting] = useState<BugReportLink | null>(null)
  const refresh = () => queryClient.invalidateQueries({ queryKey: bugReportKeys.links(projectId) })

  const toggle = useMutation({
    mutationFn: (link: BugReportLink) => updateBugReportLink(link.id, { state: link.state === 'active' ? 'paused' : 'active' }),
    onSuccess: (link) => showToast({ title: link.state === 'active' ? `${link.name} is on` : `${link.name} paused` }),
    onError: (error) => toastError('The link didn’t change', error),
    onSettled: refresh,
  })
  const replace = useMutation({
    mutationFn: (link: BugReportLink) => replaceBugReportLink(link.id),
    onSuccess: (result) => {
      setReplacing(null)
      setSetup({ name: result.link.name, url: result.url })
    },
    onSettled: refresh,
  })
  const remove = useMutation({
    mutationFn: (link: BugReportLink) => deleteBugReportLink(link.id),
    onSuccess: (_, link) => {
      setDeleting(null)
      showToast({ title: `${link.name} deleted` })
    },
    onSettled: refresh,
  })

  const canCreate = list.data?.canCreate === true
  // A collection's forms are links too, but they're managed from that collection.
  const links = list.data?.links.filter((link) => !link.collectionId) ?? []

  return (
    <>
      <PageHeader
        actions={
          canCreate ? (
            <Button icon="plus" onClick={() => setCreating(true)} variant="primary">
              New link
            </Button>
          ) : null
        }
        icon="circle-alert"
        title="Bug reports"
      >
        Give another app a link, and every bug report it sends becomes a task in {project.name}. Reports are added as the person who made the link. Forms that add records are shared from their collection.
      </PageHeader>
      {setup && <SetupCard name={setup.name} onDone={() => setSetup(null)} projectName={project.name} url={setup.url} />}
      {list.isPending ? (
        <ListSkeleton label="Loading bug report links" rows={2} />
      ) : list.isError ? (
        <ErrorState error={list.error} onRetry={() => void list.refetch()} title="Bug report links didn’t load" />
      ) : (
        <Section id="bug-report-links-heading" title="Links">
          {links.length ? (
            <RowList label="Bug report links">
              {links.map((link) => (
                <Row
                  actions={
                    link.canManage ? (
                      <Menu
                        items={[
                          {
                            label: link.state === 'active' ? 'Pause' : 'Turn on',
                            icon: link.state === 'active' ? 'stop' : 'play',
                            onSelect: () => toggle.mutate(link),
                          },
                          { label: 'Replace the link', icon: 'key', onSelect: () => setReplacing(link) },
                          { separator: true },
                          { label: 'Delete', icon: 'trash', danger: true, onSelect: () => setDeleting(link) },
                        ]}
                        label={`${link.name} actions`}
                        trigger={<IconButton icon="more" label={`${link.name} actions`} size="sm" />}
                      />
                    ) : null
                  }
                  className={link.state === 'active' ? undefined : 'is-disabled'}
                  key={link.id}
                  leading={
                    <span className="aw-row__tile">
                      <Icon name={Bug} size={16} />
                    </span>
                  }
                  meta={
                    <span className="aw-row__metaline aw-wrap">
                      <span>
                        Ends in <code className="aw-inline-code">…{link.tokenHint}</code>
                      </span>
                      <span>{countLabel(link.reportCount, 'report')}</span>
                      <span title={link.lastReportAt ? formatFullDate(link.lastReportAt) : undefined}>
                        {link.lastReportAt ? `Last report ${formatRelative(link.lastReportAt)}` : 'No reports yet'}
                      </span>
                      {link.ownerName && <span>Made by {link.ownerName}</span>}
                      {link.disabledReason && <span>{link.disabledReason}</span>}
                    </span>
                  }
                  title={
                    <span className="aw-row__titleline">
                      {link.name}
                      <StatusChip tone={STATE[link.state].tone}>{STATE[link.state].label}</StatusChip>
                    </span>
                  }
                />
              ))}
            </RowList>
          ) : (
            <EmptyState
              action={
                canCreate ? (
                  <Button icon="plus" onClick={() => setCreating(true)}>
                    New link
                  </Button>
                ) : null
              }
              icon="circle-alert"
              title="No bug report links yet"
            >
              Make one for each app, such as a game or a website, then paste its setup into that app. Crashes and reports arrive here as tasks.
            </EmptyState>
          )}
        </Section>
      )}
      {creating && (
        <CreateDialog
          onClose={() => setCreating(false)}
          onCreated={(name, url) => {
            setCreating(false)
            setSetup({ name, url })
            void refresh()
          }}
          projectId={projectId}
        />
      )}
      <ConfirmDialog
        busy={replace.isPending}
        confirmLabel="Replace link"
        error={replace.isError ? `The link wasn’t replaced. ${describeError(replace.error).message}` : null}
        onClose={() => {
          setReplacing(null)
          replace.reset()
        }}
        onConfirm={() => replacing && replace.mutate(replacing)}
        open={replacing !== null}
        title={`Replace the link for ${replacing?.name ?? 'this app'}?`}
      >
        The old link stops working right away. Reports only arrive again once the app has the new one.
      </ConfirmDialog>
      <ConfirmDialog
        busy={remove.isPending}
        confirmLabel="Delete link"
        error={remove.isError ? `The link wasn’t deleted. ${describeError(remove.error).message}` : null}
        onClose={() => {
          setDeleting(null)
          remove.reset()
        }}
        onConfirm={() => deleting && remove.mutate(deleting)}
        open={deleting !== null}
        title={`Delete ${deleting?.name ?? 'this link'}?`}
      >
        It stops taking reports right away. Tasks it already made stay.
      </ConfirmDialog>
    </>
  )
}

function CreateDialog({
  onClose,
  onCreated,
  projectId,
}: {
  onClose: () => void
  onCreated: (name: string, url: string) => void
  projectId: string
}) {
  const [name, setName] = useState('')
  const [error, setError] = useState<string | null>(null)
  const save = useMutation({
    mutationFn: () => createBugReportLink(projectId, name.trim()),
    onSuccess: (result) => onCreated(result.link.name, result.url),
    onError: (reason) => setError(`The link wasn’t made. ${describeError(reason).message}`),
  })
  const submit = (event: FormEvent<HTMLFormElement>) => {
    event.preventDefault()
    if (name.trim()) save.mutate()
  }
  return (
    <Dialog
      dirty={name !== ''}
      footer={
        <>
          <Button onClick={onClose}>Cancel</Button>
          <Button busy={save.isPending} disabled={!name.trim()} type="submit" variant="primary">
            Make link
          </Button>
        </>
      }
      onClose={onClose}
      onSubmit={submit}
      open
      title="New bug report link"
    >
      <div className="aw-form">
        <TextField
          hint="The app that sends reports, such as a game or a website. It’s shown on each task."
          id="bug-report-link-name"
          label="App name"
          maxLength={80}
          onChange={(event) => setName(event.target.value)}
          required
          value={name}
        />
        {error && <Banner tone="danger">{error}</Banner>}
      </div>
    </Dialog>
  )
}

/** The new link and how to set it up, shown once. */
function SetupCard({ name, onDone, projectName, url }: { name: string; onDone: () => void; projectName: string; url: string }) {
  const [kind, setKind] = useState<SnippetKind>('agent')
  const [copied, setCopied] = useState<string | null>(null)
  const timer = useRef<number | undefined>(undefined)
  const heading = useRef<HTMLHeadingElement>(null)
  useEffect(() => {
    heading.current?.focus()
    return () => window.clearTimeout(timer.current)
  }, [])
  const text = snippet(kind, url, name, projectName)
  const copy = (what: string, value: string) => {
    if (!navigator.clipboard) {
      setCopied('failed')
      return
    }
    navigator.clipboard.writeText(value).then(
      () => {
        setCopied(what)
        window.clearTimeout(timer.current)
        timer.current = window.setTimeout(() => setCopied(null), 2000)
      },
      () => setCopied('failed'),
    )
  }
  return (
    <aside aria-label={`Set up ${name}`} className="aw-card aw-token">
      <div className="aw-token__head">
        <Icon name={Bug} size={18} />
        <h3 ref={heading} tabIndex={-1}>
          Set up {name}
        </h3>
      </div>
      <p>
        Anyone with this link can add tasks here, so keep it out of public code. It can’t be shown again; replace it if you lose it.
      </p>
      <code className="aw-token__value">{url}</code>
      <p>
        The easiest way: copy the instructions below into an agent working on {name}, and it adds crash and bug reporting for you. Or copy the code
        yourself.
      </p>
      <SegmentedControl label="Setup for" onChange={setKind} options={SNIPPET_KINDS} value={kind} />
      <pre className="aw-code aw-code--wrap" tabIndex={0}>
        {text}
      </pre>
      <div className="aw-card__actions aw-onetime__actions">
        <Button icon={copied === 'snippet' ? 'check' : 'copy'} onClick={() => copy('snippet', text)} variant="primary">
          {copied === 'snippet' ? 'Copied' : kind === 'agent' ? 'Copy instructions' : 'Copy code'}
        </Button>
        <Button icon={copied === 'url' ? 'check' : 'copy'} onClick={() => copy('url', url)}>
          {copied === 'url' ? 'Copied' : 'Copy link only'}
        </Button>
        <Button onClick={onDone}>Done</Button>
      </div>
      {copied === 'failed' ? (
        <p className="aw-quiet" role="status">
          Couldn’t copy. Select the text above and copy it yourself.
        </p>
      ) : null}
    </aside>
  )
}
