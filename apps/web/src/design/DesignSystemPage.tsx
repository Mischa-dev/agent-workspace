import { useState } from 'react'
import { Link } from 'react-router-dom'

import { useAuthenticatedSession } from '../auth/session'
import { AppShell } from '../shell/AppShell'
import { THEME_PRESETS } from '../theme/slots'
import { useTheme } from '../theme/themeContext'
import {
  Avatar,
  ActorTag,
  Badge,
  Banner,
  Button,
  Checkbox,
  DetailList,
  DocumentSkeleton,
  EmptyState,
  ErrorState,
  Icon,
  IconButton,
  ListSkeleton,
  MessageSkeleton,
  PageHeader,
  Kbd,
  PageSkeleton,
  Row,
  RowList,
  Section,
  SegmentedControl,
  SelectField,
  SettingsSkeleton,
  StatusChip,
  Switch,
  TextArea,
  TextField,
  formatDateTime,
  formatRelative,
  relativeDay,
  showToast,
  type BannerTone,
  type ChipTone,
} from '../ui'
import { CommandMenu } from '../ui/commandMenu'
import { Markdown, MarkdownEditor } from '../ui/markdown'
import { TokenSpecimens } from './TokenSpecimens'
import './design.css'

const SAMPLE_MARKDOWN = `## Ship the toast
- [x] Toast with Undo
- [ ] Wire archive to it, see AWD-12

\`\`\`ts
showToast({ title: 'Archived', onUndo: restore })
\`\`\`

> Errors are toasts, never "reload the page".`

const BANNER_TONES: BannerTone[] = ['info', 'success', 'warning', 'danger', 'agent', 'offline']
const CHIP_TONES: ChipTone[] = ['muted', 'agent', 'warning', 'success', 'danger']

/** The design system, live: every token and shared component in the current theme. apps/web/DESIGN.md says when to use each. */
export function DesignSystemPage() {
  const session = useAuthenticatedSession()
  const theme = useTheme()
  const [menuOpen, setMenuOpen] = useState(false)
  const [draft, setDraft] = useState(SAMPLE_MARKDOWN)
  const [checked, setChecked] = useState(true)
  const [view, setView] = useState<'list' | 'board'>('list')
  const [times] = useState(() => {
    const now = Date.now()
    return [now - 20_000, now - 5 * 60_000, now - 3 * 3_600_000, now - 2 * 86_400_000, now - 40 * 86_400_000].map((at) => new Date(at).toISOString())
  })

  return (
    <AppShell mainClassName="aw-main--page" user={session.user}>
      <div className="aw-page aw-page--wide aw-ds">
        <PageHeader icon="palette" title="Design system">
          The tokens and shared components every page is built from, in the current theme. Pick a theme to check each one; your own choice
          stays on <Link to="/settings/appearance">Appearance</Link>.
        </PageHeader>
        <div aria-label="Theme" className="aw-ds__themes" role="group">
          {THEME_PRESETS.map((preset) => (
            <Button
              aria-pressed={theme.preset === preset.id}
              key={preset.id}
              onClick={() => theme.setTheme(preset.id)}
              size="sm"
              variant={theme.preset === preset.id ? 'primary' : 'secondary'}
            >
              {preset.name}
            </Button>
          ))}
        </div>

        <TokenSpecimens />

        <Section
          description="One primary action per view. Secondary for the rest, ghost inside rows, toolbars and section headers, danger only to confirm what can't be undone."
          title="Buttons"
        >
          {(['primary', 'secondary', 'ghost', 'danger'] as const).map((variant) => (
            <div className="aw-ds__specimen" key={variant}>
              <span className="aw-ds__label aw-ds__rowlabel">{variant}</span>
              <Button size="sm" variant={variant}>
                Small
              </Button>
              <Button icon="plus" variant={variant}>
                Medium
              </Button>
              <Button size="lg" variant={variant}>
                Large
              </Button>
              <Button busy variant={variant}>
                Busy
              </Button>
              <Button disabled variant={variant}>
                Disabled
              </Button>
            </div>
          ))}
          <div className="aw-ds__specimen">
            <span className="aw-ds__label aw-ds__rowlabel">icon</span>
            <IconButton icon="settings" label="Settings" />
            <IconButton icon="star" label="Pinned" pressed />
            <IconButton icon="more" label="More" size="sm" />
            <IconButton disabled icon="trash" label="Delete" />
            <Kbd>⌘K</Kbd>
          </div>
        </Section>

        <Section description="Fields are 40px in forms and 32px (compact) in toolbars. A hint sits under the field; an error replaces it." title="Fields and choices">
          <div className="aw-ds__specimen aw-ds__grid">
            <TextField hint="People see this in messages." label="Display name" placeholder="Muse" />
            <TextField error="Use letters, numbers and dashes." label="Handle" prefix="@" defaultValue="muse bot" />
            <TextField disabled label="Key" mono value="SMK" readOnly />
            <SelectField label="Role" defaultValue="member">
              <option value="member">Member</option>
              <option value="admin">Admin</option>
            </SelectField>
            <TextField compact aria-label="Search" placeholder="Search by title or key" prefix={<Icon name="search" size={14} />} />
            <TextArea label="Description" placeholder="What needs doing?" rows={2} />
          </div>
          <div className="aw-ds__specimen aw-ds__grid">
            <Checkbox checked={checked} description="Ticked boxes save with the form." label="Include archived tasks" onChange={setChecked} />
            <Switch checked={checked} description="Switches apply at once." label="Notify me" onChange={setChecked} />
            <SegmentedControl
              label="View"
              onChange={setView}
              options={[
                { value: 'list', label: 'List', icon: 'list' },
                { value: 'board', label: 'Board', icon: 'board' },
              ]}
              value={view}
            />
          </div>
        </Section>

        <Section
          actions={
            <Button icon="plus" size="sm" variant="ghost">
              Section action
            </Button>
          }
          description="Bordered lists and cards share one surface: raised, 1px border, md corners, the card shadow. Never put a card inside a card; use rows."
          title="Rows, cards and details"
        >
          <RowList label="Sample rows">
            <Row
              actions={
                <Button size="sm" variant="ghost">
                  Open
                </Button>
              }
              leading={<Avatar name={session.user.displayName} seed={session.user.actorId} size={32} />}
              meta="@smoke · Joined today"
              title={
                <>
                  {session.user.displayName} <StatusChip tone="success">Owner</StatusChip>
                </>
              }
            />
            <Row leading={<Avatar kind="agent" name="Muse" seed="muse" size={32} />} meta="Writes release notes" title={<>Muse <ActorTag kind="agent" /></>} />
          </RowList>
          <DetailList
            items={[
              { label: 'Key', value: <code>SMK</code> },
              { label: 'Your role', value: 'Owner' },
            ]}
          />
        </Section>

        <Section title="Toasts">
          <div className="aw-ds__specimen">
            <Button onClick={() => showToast({ tone: 'success', title: 'Saved' })}>Success</Button>
            <Button onClick={() => showToast({ title: 'Archived #design', onUndo: () => new Promise((resolve) => window.setTimeout(resolve, 600)) })}>
              With Undo
            </Button>
            <Button
              onClick={() =>
                showToast({ tone: 'agent', title: 'Run finished', description: 'The agent opened a pull request.', action: { label: 'Open PR', onSelect: () => undefined } })
              }
            >
              Background result
            </Button>
            <Button onClick={() => showToast({ tone: 'danger', title: 'Couldn’t save the task.', description: 'Someone changed it first. Your edit is still here.' })}>
              Error
            </Button>
          </div>
        </Section>

        <Section title="Command menu">
          <div className="aw-ds__specimen">
            <Button onClick={() => setMenuOpen(true)}>Open a sample menu</Button>
          </div>
          {menuOpen ? (
            <CommandMenu
              items={[
                { id: 'home', group: 'Go to', label: 'Home', icon: 'home', shortcut: 'G H', onSelect: () => undefined },
                { id: 'tasks', group: 'Go to', label: 'Tasks', icon: 'checklist', shortcut: 'G T', onSelect: () => undefined },
                { id: 'new', group: 'Actions', label: 'Create task', icon: 'plus', shortcut: 'C', onSelect: () => undefined },
              ]}
              label="Sample command menu"
              onClose={() => setMenuOpen(false)}
            />
          ) : null}
        </Section>

        <Section title="Banners and chips">
          <div className="aw-ds__specimen aw-ds__specimen--stack">
            {BANNER_TONES.map((tone) => (
              <Banner key={tone} title={`${tone[0].toUpperCase()}${tone.slice(1)}.`} tone={tone}>
                A note about this pane.
              </Banner>
            ))}
          </div>
          <div className="aw-ds__specimen">
            {CHIP_TONES.map((tone) => (
              <StatusChip key={tone} tone={tone}>
                {tone}
              </StatusChip>
            ))}
            <Badge count={3} />
            <Badge count={120} max={99} />
            <Avatar name={session.user.displayName} presence="online" seed={session.user.actorId} size={32} />
          </div>
        </Section>

        <Section title="Skeletons">
          <div className="aw-ds__specimen aw-ds__specimen--stack">
            <ListSkeleton decorative rows={3} />
            <SettingsSkeleton decorative sections={1} />
            <PageSkeleton decorative cards={2} />
            <DocumentSkeleton decorative paragraphs={1} />
            <MessageSkeleton decorative rows={2} />
          </div>
        </Section>

        <Section title="Empty and error states">
          <div className="aw-ds__specimen">
            <EmptyState action={<Button size="sm">Create task</Button>} icon="checklist" title="No tasks yet">
              Tasks you and your agents create show up here.
            </EmptyState>
            <ErrorState action={<Button size="sm">Try again</Button>} title="This didn’t load">
              The server couldn’t be reached.
            </ErrorState>
          </div>
        </Section>

        <Section title="Markdown">
          <div className="aw-ds__specimen aw-ds__specimen--stack">
            <MarkdownEditor label="Description" onChange={setDraft} value={draft} />
            <Markdown source={draft} />
          </div>
        </Section>

        <Section title="Dates and times">
          <dl className="aw-ds__specimen aw-ds__specimen--stack">
            {times.map((iso) => (
              <div key={iso}>
                <dt>{formatRelative(iso)}</dt>
                <dd>
                  {relativeDay(iso)} · {formatDateTime(iso)}
                </dd>
              </div>
            ))}
          </dl>
        </Section>
      </div>
    </AppShell>
  )
}
