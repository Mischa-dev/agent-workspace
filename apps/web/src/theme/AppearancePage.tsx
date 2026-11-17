import { useQuery, useQueryClient } from '@tanstack/react-query'
import { Download, Upload } from 'lucide-react'
import { Suspense, lazy, useEffect, useRef, useState, type CSSProperties } from 'react'

import type { ThemeDocument, ThemeSummary } from '../api/generated/operations'
import { callOp, invalidateOps, opQuery } from '../api/ops'
import {
  ActorTag,
  Avatar,
  Badge,
  Banner,
  Button,
  ButtonLink,
  ConfirmDialog,
  Dots,
  Icon,
  IconButton,
  Menu,
  PageHeader,
  Section,
  SegmentedControl,
  SettingsSkeleton,
  describeError,
  formatTime,
  showToast,
  type MenuEntry,
} from '../ui'
import type { SettingsPageProps } from '../settings/types'
import { blankDocument, readDocument } from './contract'
import type { EditorStart } from './ThemeEditor'
import { THEME_PRESETS, isDarkScheme, loadThemeSheets } from './slots'
import type { AppearancePreferences, ThemeChoice } from './preferences'
import { useTheme } from './themeContext'
import '../styles/conversation.css'
import '../styles/pages.css'

const PRESET_GROUPS = [
  { id: 'workspace', label: 'Agent Workspace' },
  { id: 'dark', label: 'Classic dark' },
  { id: 'light', label: 'Classic light' },
] as const

function PresetMini({ id }: { id: string }) {
  return (
    <span aria-hidden="true" className="aw-mini" data-theme={id}>
      <span className="aw-mini__rail" />
      <span className="aw-mini__side">
        <i />
        <i />
        <i />
      </span>
      <span className="aw-mini__main">
        <i />
        <i />
        <b />
      </span>
    </span>
  )
}

/** A time today, as the clock on this device shows it: "10:24 AM" or "10:24". */
function todayAt(hours: number, minutes: number) {
  const date = new Date()
  date.setHours(hours, minutes, 0, 0)
  return formatTime(date.toISOString())
}

/** A small, real rendering of the shell so the sidebar and rail colors show on this page too. */
function ThemePreview({ name, seed }: { name: string; seed: string }) {
  return (
    <div aria-hidden="true" className="aw-preview">
      <div className="aw-preview__rail" data-region="rail">
        <span className="aw-tile aw-tile--utility is-active">
          <span className="aw-tile__indicator" />
          <span className="aw-tile__face">
            <Icon name="message" size={16} />
          </span>
        </span>
        <span className="aw-tile aw-tile--space is-unread">
          <span className="aw-tile__indicator" />
          <span className="aw-tile__face" style={{ '--tile-bg': 'var(--swatch-indigo)' } as CSSProperties}>
            AW
          </span>
          <Badge className="aw-tile__badge" count={2} />
        </span>
        <span className="aw-tile aw-tile--space">
          <span className="aw-tile__face" style={{ '--tile-bg': 'var(--swatch-teal)' } as CSSProperties}>
            HL
          </span>
        </span>
      </div>
      <div className="aw-preview__side" data-region="sidebar">
        <div className="aw-preview__title">Agent Workspace</div>
        <span className="aw-item">
          <span className="aw-item__lead">
            <Icon name="hash" />
          </span>
          <span className="aw-item__label">general</span>
        </span>
        <span className="aw-item is-active">
          <span className="aw-item__lead">
            <Icon name="hash" />
          </span>
          <span className="aw-item__label">design</span>
        </span>
        <span className="aw-item is-unread">
          <span className="aw-item__lead">
            <Icon name="hash" />
          </span>
          <span className="aw-item__label">deploys</span>
          <span className="aw-item__trail">
            <Badge count={3} />
          </span>
        </span>
        <span className="aw-item">
          <span className="aw-item__lead">
            <Avatar kind="agent" name="Muse" size={20} />
          </span>
          <span className="aw-item__label">Muse</span>
          <span className="aw-item__trail">
            <Dots />
          </span>
        </span>
      </div>
      <div className="aw-preview__main" data-region="content">
        <div className="aw-preview__msg">
          <Avatar name={name} seed={seed} size={28} />
          <div>
            <div className="aw-msg__meta">
              <span className="aw-msg__author">{name}</span>
              <span className="aw-msg__time">{todayAt(10, 24)}</span>
            </div>
            <p className="aw-preview__text">
              <span className="aw-mention">@Muse</span> can you check the new colors? <a href="#preview">Link</a>
            </p>
          </div>
        </div>
        <div className="aw-preview__msg is-highlighted">
          <Avatar kind="agent" name="Muse" size={28} />
          <div>
            <div className="aw-msg__meta">
              <span className="aw-msg__author">Muse</span>
              <ActorTag kind="agent" />
              <span className="aw-msg__time">{todayAt(10, 27)}</span>
            </div>
            <p className="aw-preview__text">
              Looks good. <span className="aw-mention is-you">@you</span>, the unread badge reads clearly.
            </p>
          </div>
        </div>
        <div className="aw-preview__composer">
          <span>Message #design</span>
          <span className="aw-composer__send">
            <Icon name="send" />
          </span>
        </div>
      </div>
    </div>
  )
}

const ThemeEditor = lazy(() => import('./ThemeEditor'))

/** A theme file: the document with its name, as Export writes and Import reads. */
function download(name: string, document: ThemeDocument) {
  const blob = new Blob([JSON.stringify({ name, ...document }, null, 2)], {
    type: 'application/json',
  })
  const link = window.document.createElement('a')
  link.href = URL.createObjectURL(blob)
  link.download = `${
    name
      .replace(/[^\p{L}\p{N}]+/gu, '-')
      .replace(/^-|-$/g, '')
      .toLowerCase() || 'theme'
  }.theme.json`
  link.click()
  window.setTimeout(() => URL.revokeObjectURL(link.href), 1000)
}

/** Account › Appearance in settings. */
export default function AppearancePage({ session, workspace }: SettingsPageProps) {
  const theme = useTheme()
  const queryClient = useQueryClient()
  const workspaceId = workspace?.id
  const themes = useQuery({
    ...opQuery('theme_list', {}, { workspaceId }),
    enabled: Boolean(workspaceId),
  })
  const [editing, setEditing] = useState<EditorStart | null>(null)
  const [deleting, setDeleting] = useState<ThemeSummary | null>(null)
  const [busy, setBusy] = useState(false)
  const importInput = useRef<HTMLInputElement>(null)
  // The built-in themes' previews need their stylesheets even when another theme is in use.
  useEffect(() => loadThemeSheets(), [])

  const mine = themes.data?.themes.filter((entry) => entry.mine) ?? []
  const offered = themes.data?.themes.filter((entry) => !entry.mine && entry.offered) ?? []
  const canOffer = themes.data?.canOffer === true
  const using = theme.customTheme?.id ?? null

  const current = (): AppearancePreferences => ({
    theme: theme.theme,
    customTheme: theme.customTheme,
    mode: theme.mode,
    overrides: theme.overrides,
    density: theme.density,
    textSize: theme.textSize,
    sidebarWidth: theme.sidebarWidth,
    panelWidth: theme.panelWidth,
  })
  const refresh = () => invalidateOps(queryClient, ['theme_list', 'theme_get'], workspaceId)
  const read = async (id: string) => callOp('theme_get', { id }, { workspaceId })
  const failed = (title: string) => (reason: unknown) => showToast({ title, description: describeError(reason).message, tone: 'danger' })

  const choose = (next: ThemeChoice) => {
    if (next === theme.theme && !using) return
    const previous = current()
    theme.setTheme(next)
    if (previous.customTheme)
      showToast({
        title: 'Back to a built-in theme',
        onUndo: () => theme.replaceAppearance(previous),
      })
  }
  const use = async (entry: ThemeSummary) => {
    if (!workspaceId) return
    try {
      const found = await read(entry.id)
      theme.setCustomTheme({
        id: found.id,
        workspaceId,
        version: found.version,
        name: found.name,
        document: found.document,
      })
    } catch (reason) {
      failed('That theme couldn’t be opened')(reason)
    }
  }
  const startNew = () => {
    const light = theme.customDocument?.document.light.base ?? (isDarkScheme(theme.preset) ? 'quiet' : theme.preset)
    const dark = theme.customDocument?.document.dark.base ?? (isDarkScheme(theme.preset) ? theme.preset : 'quiet-dark')
    setEditing({
      id: null,
      version: 0,
      name: 'My theme',
      document: blankDocument(light, dark),
    })
  }
  const edit = async (entry: ThemeSummary) => {
    try {
      const found = await read(entry.id)
      setEditing({
        id: found.id,
        version: found.version,
        name: found.name,
        document: found.document,
      })
    } catch (reason) {
      failed('That theme couldn’t be opened')(reason)
    }
  }
  const duplicate = async (entry: ThemeSummary) => {
    try {
      const found = await read(entry.id)
      setEditing({
        id: null,
        version: 0,
        name: `${found.name} copy`.slice(0, 60),
        document: found.document,
      })
    } catch (reason) {
      failed('That theme couldn’t be opened')(reason)
    }
  }
  const exportTheme = async (entry: ThemeSummary) => {
    try {
      const found = await read(entry.id)
      download(found.name, found.document)
    } catch (reason) {
      failed('That theme couldn’t be exported')(reason)
    }
  }
  const importFile = async (file: File) => {
    if (!workspaceId) return
    let parsed: unknown
    try {
      parsed = JSON.parse(await file.text())
    } catch {
      parsed = null
    }
    const document = readDocument(parsed)
    if (!document) {
      showToast({
        title: 'That file isn’t a theme this app can use',
        description: 'Export a theme here and import that file.',
        tone: 'danger',
      })
      return
    }
    const named = (parsed as { name?: unknown }).name
    const name = (typeof named === 'string' && named.trim() ? named.trim() : file.name.replace(/\.theme\.json$|\.json$/, '')).slice(0, 60)
    try {
      const created = await callOp('theme_create', { name, document }, { workspaceId })
      theme.setCustomTheme({
        id: created.id,
        workspaceId,
        version: created.version,
        name: created.name,
        document: created.document,
      })
      await refresh()
      showToast({ title: `Using ${created.name}` })
    } catch (reason) {
      failed('That theme couldn’t be imported')(reason)
    }
  }
  const offer = async (entry: ThemeSummary, next: boolean) => {
    try {
      await callOp('workspace_theme_set', { themeId: entry.id, offered: next }, { workspaceId })
      await refresh()
      showToast({
        title: next ? `${entry.name} is offered to everyone here` : `${entry.name} is yours alone again`,
      })
    } catch (reason) {
      failed('That didn’t change')(reason)
    }
  }
  const remove = async (entry: ThemeSummary) => {
    setBusy(true)
    try {
      await callOp('theme_delete', { id: entry.id }, { workspaceId })
      if (using === entry.id) theme.setTheme(theme.theme)
      await refresh()
      setDeleting(null)
    } catch (reason) {
      failed('That theme wasn’t deleted')(reason)
    } finally {
      setBusy(false)
    }
  }
  const reset = () => {
    const previous = current()
    theme.resetAppearance()
    showToast({
      title: 'Appearance reset',
      onUndo: () => theme.replaceAppearance(previous),
    })
  }

  const actions = (entry: ThemeSummary): MenuEntry[] => [
    ...(entry.mine
      ? [
          {
            label: 'Edit',
            icon: 'pencil' as const,
            onSelect: () => void edit(entry),
          },
        ]
      : []),
    { label: 'Duplicate', icon: 'copy', onSelect: () => void duplicate(entry) },
    {
      label: 'Export as a file',
      icon: Download,
      onSelect: () => void exportTheme(entry),
    },
    ...(canOffer
      ? [
          entry.offered
            ? {
                label: 'Stop offering to the workspace',
                icon: 'users' as const,
                onSelect: () => void offer(entry, false),
              }
            : {
                label: 'Offer to everyone in the workspace',
                icon: 'users' as const,
                onSelect: () => void offer(entry, true),
              },
        ]
      : []),
    ...(entry.mine || canOffer
      ? [
          { separator: true as const },
          {
            label: 'Delete',
            icon: 'trash' as const,
            danger: true,
            onSelect: () => setDeleting(entry),
          },
        ]
      : []),
  ]

  const themeRows = (entries: ThemeSummary[]) => (
    <div className="aw-presets">
      {entries.map((entry) => (
        <div className="aw-preset aw-preset--custom" key={entry.id}>
          <button aria-checked={using === entry.id} className="aw-preset__pick" onClick={() => void use(entry)} role="radio" type="button">
            <span aria-hidden="true" className="aw-mini aw-mini--split">
              <PresetMini id={entry.lightBase} />
              <PresetMini id={entry.darkBase} />
            </span>
            <span className="aw-preset__name">{entry.name}</span>
          </button>
          <Menu
            items={actions(entry)}
            label={`${entry.name} actions`}
            trigger={<IconButton icon="more" label={`${entry.name} actions`} size="sm" />}
          />
        </div>
      ))}
    </div>
  )

  return (
    <div className="aw-appearance">
      <PageHeader
        actions={
          <>
            <ButtonLink to="/design-system" variant="ghost">
              See every component
            </ButtonLink>
            <Button icon="undo" onClick={reset} variant="ghost">
              Reset everything
            </Button>
          </>
        }
        icon="palette"
        title="Appearance"
      >
        Pick a built-in theme or make your own: every colour, font, size, spacing and shape, for light and dark, and CSS. Your choice follows you to every
        device you sign in on.
      </PageHeader>

      <div className="aw-appearance__layout">
        <div className="aw-appearance__controls">
          {editing && workspaceId ? (
            <Suspense fallback={<SettingsSkeleton />}>
              <ThemeEditor onDone={() => setEditing(null)} start={editing} workspaceId={workspaceId} />
            </Suspense>
          ) : (
            <>
              <Section
                actions={
                  workspaceId ? (
                    <>
                      <Button icon="plus" onClick={startNew} size="sm">
                        New theme
                      </Button>
                      <Button icon={Upload} onClick={() => importInput.current?.click()} size="sm" variant="ghost">
                        Import
                      </Button>
                      <input
                        accept="application/json,.json"
                        hidden
                        onChange={(event) => {
                          const file = event.target.files?.[0]
                          event.target.value = ''
                          if (file) void importFile(file)
                        }}
                        ref={importInput}
                        type="file"
                      />
                    </>
                  ) : null
                }
                description="Themes you made, and ones this workspace offers to everyone. A theme sets light and dark; which shows follows Mode below."
                title="Your themes"
              >
                {themes.isError ? <Banner tone="danger">{describeError(themes.error).message}</Banner> : null}
                {mine.length ? (
                  <div aria-label="Your themes" role="radiogroup">
                    {themeRows(mine)}
                  </div>
                ) : (
                  <p className="aw-appearance__hint">None yet. New theme starts from the one you’re using.</p>
                )}
                {offered.length ? (
                  <div aria-labelledby="aw-presets-offered" className="aw-preset-group" role="radiogroup">
                    <h3 className="aw-preset-group__title" id="aw-presets-offered">
                      Offered in this workspace
                    </h3>
                    {themeRows(offered)}
                  </div>
                ) : null}
                {using ? (
                  <div className="aw-appearance__row">
                    <div>
                      <p className="aw-appearance__label">Mode</p>
                      <p className="aw-appearance__hint">Which half of {theme.customDocument?.name ?? 'the theme'} shows.</p>
                    </div>
                    <SegmentedControl
                      label="Mode"
                      onChange={theme.setMode}
                      options={[
                        { value: 'system', label: 'Match device' },
                        { value: 'light', label: 'Light' },
                        { value: 'dark', label: 'Dark' },
                      ]}
                      value={theme.mode}
                    />
                  </div>
                ) : null}
              </Section>
              <Section title="Built-in themes">
                <div aria-label="Theme" className="aw-preset-groups" role="radiogroup">
                  {PRESET_GROUPS.map((group) => (
                    <div aria-labelledby={`aw-presets-${group.id}`} className="aw-preset-group" key={group.id} role="group">
                      <h3 className="aw-preset-group__title" id={`aw-presets-${group.id}`}>
                        {group.label}
                      </h3>
                      <div className="aw-presets">
                        {THEME_PRESETS.filter((preset) => preset.group === group.id).map((preset) => (
                          <button
                            aria-checked={!using && theme.theme === preset.id}
                            className="aw-preset"
                            key={preset.id}
                            onClick={() => choose(preset.id)}
                            role="radio"
                            type="button"
                          >
                            <PresetMini id={preset.id} />
                            <span className="aw-preset__name">{preset.name}</span>
                          </button>
                        ))}
                        {group.id === 'workspace' ? (
                          <button
                            aria-checked={!using && theme.theme === 'system'}
                            className="aw-preset"
                            onClick={() => choose('system')}
                            role="radio"
                            type="button"
                          >
                            <span aria-hidden="true" className="aw-mini aw-mini--split">
                              <PresetMini id="quiet" />
                              <PresetMini id="quiet-dark" />
                            </span>
                            <span className="aw-preset__name">Match system</span>
                          </button>
                        ) : null}
                      </div>
                    </div>
                  ))}
                </div>
              </Section>

              <Section title="Density and text">
                <div className="aw-appearance__row">
                  <div>
                    <p className="aw-appearance__label">Density</p>
                    <p className="aw-appearance__hint">Compact fits more messages and rows on screen.</p>
                  </div>
                  <SegmentedControl
                    label="Density"
                    onChange={theme.setDensity}
                    options={[
                      { value: 'comfortable', label: 'Comfortable' },
                      { value: 'compact', label: 'Compact' },
                    ]}
                    value={theme.density}
                  />
                </div>
                <div className="aw-appearance__row">
                  <div>
                    <p className="aw-appearance__label">Text size</p>
                    <p className="aw-appearance__hint">Scales messages, the sidebar and menus together.</p>
                  </div>
                  <SegmentedControl
                    label="Text size"
                    onChange={theme.setTextSize}
                    options={[
                      { value: 'small', label: 'Small' },
                      { value: 'default', label: 'Default' },
                      { value: 'large', label: 'Large' },
                    ]}
                    value={theme.textSize}
                  />
                </div>
              </Section>
            </>
          )}
        </div>
        <div className="aw-appearance__preview">
          <p className="aw-appearance__label">Preview</p>
          <ThemePreview name={session.user.displayName} seed={session.user.actorId} />
        </div>
      </div>
      <ConfirmDialog
        busy={busy}
        confirmLabel="Delete theme"
        onClose={() => setDeleting(null)}
        onConfirm={() => deleting && void remove(deleting)}
        open={deleting !== null}
        title={`Delete ${deleting?.name ?? 'this theme'}?`}
      >
        Anyone using it goes back to the built-in theme it started from.
      </ConfirmDialog>
    </div>
  )
}
