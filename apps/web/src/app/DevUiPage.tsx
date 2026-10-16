import { useState } from 'react'

import { useAuthenticatedSession } from '../auth/session'
import { AppShell } from '../shell/AppShell'
import { THEME_PRESETS } from '../theme/slots'
import { useTheme } from '../theme/themeContext'
import {
  Avatar,
  Badge,
  Banner,
  Button,
  DocumentSkeleton,
  EmptyState,
  ErrorState,
  IconButton,
  ListSkeleton,
  MessageSkeleton,
  PageHeader,
  PageSkeleton,
  Section,
  SettingsSkeleton,
  StatusChip,
  formatDateTime,
  formatRelative,
  relativeDay,
  showToast,
  type BannerTone,
  type ChipTone,
} from '../ui'
import { CommandMenu } from '../ui/commandMenu'
import { Markdown, MarkdownEditor } from '../ui/markdown'

const SAMPLE_MARKDOWN = `## Ship the toast
- [x] Toast with Undo
- [ ] Wire archive to it, see AWD-12

\`\`\`ts
showToast({ title: 'Archived', onUndo: restore })
\`\`\`

> Errors are toasts, never "reload the page".`

const BANNER_TONES: BannerTone[] = ['info', 'success', 'warning', 'danger', 'agent', 'offline']
const CHIP_TONES: ChipTone[] = ['muted', 'agent', 'warning', 'success', 'danger']

/** Dev-only: every shared component, so a change can be checked in each theme at once. */
export function DevUiPage() {
  const session = useAuthenticatedSession()
  const theme = useTheme()
  const [menuOpen, setMenuOpen] = useState(false)
  const [draft, setDraft] = useState(SAMPLE_MARKDOWN)
  const [times] = useState(() => {
    const now = Date.now()
    return [now - 20_000, now - 5 * 60_000, now - 3 * 3_600_000, now - 2 * 86_400_000, now - 40 * 86_400_000].map((at) => new Date(at).toISOString())
  })

  return (
    <AppShell mainClassName="aw-main--page" user={session.user}>
      <div className="aw-page aw-page--wide aw-devui">
        <PageHeader icon="palette" title="UI kit">
          Shared components in the current theme. Switch themes to check each one.
        </PageHeader>
        <div aria-label="Theme" className="aw-devui__themes" role="group">
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

        <Section title="Buttons">
          <div className="aw-devui__specimen">
            <Button variant="primary">Primary</Button>
            <Button>Secondary</Button>
            <Button variant="ghost">Ghost</Button>
            <Button variant="danger">Danger</Button>
            <Button busy>Busy</Button>
            <Button disabled>Disabled</Button>
            <IconButton icon="settings" label="Settings" />
            <IconButton icon="star" label="Pinned" pressed />
          </div>
        </Section>

        <Section title="Toasts">
          <div className="aw-devui__specimen">
            <Button onClick={() => showToast({ tone: 'success', title: 'Saved' })}>Success</Button>
            <Button onClick={() => showToast({ title: 'Archived #design', onUndo: () => new Promise((resolve) => window.setTimeout(resolve, 600)) })}>
              With Undo
            </Button>
            <Button
              onClick={() =>
                showToast({ tone: 'agent', title: 'Run finished', description: 'Codex opened a pull request.', action: { label: 'Open PR', onSelect: () => undefined } })
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
          <div className="aw-devui__specimen">
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
          <div className="aw-devui__specimen aw-devui__specimen--stack">
            {BANNER_TONES.map((tone) => (
              <Banner key={tone} title={`${tone[0].toUpperCase()}${tone.slice(1)}.`} tone={tone}>
                A note about this pane.
              </Banner>
            ))}
          </div>
          <div className="aw-devui__specimen">
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
          <div className="aw-devui__specimen aw-devui__specimen--stack">
            <ListSkeleton decorative rows={3} />
            <SettingsSkeleton decorative sections={1} />
            <PageSkeleton decorative cards={2} />
            <DocumentSkeleton decorative paragraphs={1} />
            <MessageSkeleton decorative rows={2} />
          </div>
        </Section>

        <Section title="Empty and error states">
          <div className="aw-devui__specimen">
            <EmptyState action={<Button size="sm">Create task</Button>} icon="checklist" title="No tasks yet">
              Tasks you and your agents create show up here.
            </EmptyState>
            <ErrorState action={<Button size="sm">Try again</Button>} title="This didn’t load">
              The server couldn’t be reached.
            </ErrorState>
          </div>
        </Section>

        <Section title="Markdown">
          <div className="aw-devui__specimen aw-devui__specimen--stack">
            <MarkdownEditor label="Description" onChange={setDraft} value={draft} />
            <Markdown source={draft} />
          </div>
        </Section>

        <Section title="Dates and times">
          <dl className="aw-devui__specimen aw-devui__specimen--stack">
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
