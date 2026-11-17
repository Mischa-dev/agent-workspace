import { useQueryClient } from '@tanstack/react-query'
import { useEffect, useMemo, useState } from 'react'

import type { ThemeDocument } from '../api/generated/operations'
import { callOp, invalidateOps, isVersionConflict } from '../api/ops'
import { Banner, Button, IconButton, Section, SegmentedControl, SelectField, TextArea, TextField, countLabel, describeError, showToast } from '../ui'
import { LIMITS, PARTS, TOKEN_GROUPS, cssProblem, documentProblems, valueProblem, type ThemeModeName, type TokenGroup } from './contract'
import { CONTRAST_PAIRS, contrastRatio, fixContrast } from './slotEditing'
import { THEME_PRESETS, toHex, type Rgb } from './slots'
import { useTheme } from './themeContext'

/** What the editor starts from: a saved theme of yours, or a new one. */
export type EditorStart = { id: string | null; version: number; name: string; document: ThemeDocument }

const COLOR_TOKENS = TOKEN_GROUPS.filter((group) => group.kind === 'color').flatMap((group) => group.tokens)
const OPEN_GROUPS = new Set(['frame', 'content', 'accent'])

/** What every colour token resolves to on <html> right now, as sRGB. */
function useResolvedColors(revision: string) {
  const [colors, setColors] = useState<Record<string, Rgb>>({})
  useEffect(() => {
    const canvas = document.createElement('canvas').getContext('2d', { willReadFrequently: true })
    if (!canvas) return undefined
    // Read after the theme provider has applied this revision to <html>.
    const frame = window.requestAnimationFrame(() => {
      const probe = document.createElement('span')
      probe.style.position = 'absolute'
      document.body.appendChild(probe)
      const next: Record<string, Rgb> = {}
      for (const token of COLOR_TOKENS) {
        probe.style.color = `var(--${token})`
        canvas.clearRect(0, 0, 1, 1)
        canvas.fillStyle = '#000'
        canvas.fillStyle = getComputedStyle(probe).color
        canvas.fillRect(0, 0, 1, 1)
        const [r, g, b] = canvas.getImageData(0, 0, 1, 1).data
        next[token] = [r, g, b]
      }
      probe.remove()
      setColors(next)
    })
    return () => window.cancelAnimationFrame(frame)
  }, [revision])
  return colors
}

/** A token's value from the built-in theme underneath, shown as the placeholder. */
function inherited(token: string) {
  return getComputedStyle(document.documentElement).getPropertyValue(`--${token}`).trim()
}

function TokenRow({
  colors,
  group,
  mode,
  token,
  value,
  onChange,
}: {
  colors: Record<string, Rgb>
  group: TokenGroup
  mode: ThemeModeName
  token: string
  value: string | undefined
  onChange: (value: string | undefined) => void
}) {
  const problem = value === undefined ? null : valueProblem(value)
  const label = token.replace(/-/g, ' ')
  return (
    <div className={value === undefined ? 'aw-token' : 'aw-token is-set'}>
      {group.kind === 'color' ? (
        <span className="aw-slot__swatch" style={{ background: `var(--${token})` }}>
          <input
            aria-label={`${label}, ${mode}`}
            onChange={(event) => onChange(event.target.value.toLowerCase())}
            type="color"
            value={colors[token] ? toHex(colors[token]) : '#000000'}
          />
        </span>
      ) : null}
      <TextField
        compact
        error={problem ?? undefined}
        label={label}
        mono
        onChange={(event) => onChange(event.target.value === '' ? undefined : event.target.value)}
        placeholder={value === undefined ? inherited(token) : undefined}
        value={value ?? ''}
      />
      {value !== undefined ? (
        <IconButton icon="undo" label={`Use the built-in ${label}`} onClick={() => onChange(undefined)} size="sm" />
      ) : (
        <span className="aw-slot__spacer" />
      )}
    </div>
  )
}

/**
 * The theme editor: a name, each mode's built-in base and any token, and
 * CSS, previewed live on the whole app until it's saved or dropped.
 */
export default function ThemeEditor({ start, workspaceId, onDone }: { start: EditorStart; workspaceId: string; onDone: () => void }) {
  const theme = useTheme()
  const queryClient = useQueryClient()
  const [name, setName] = useState(start.name)
  const [document, setDocument] = useState<ThemeDocument>(start.document)
  const [mode, setMode] = useState<ThemeModeName>(theme.shownMode)
  const [saving, setSaving] = useState(false)
  const [error, setError] = useState<string | null>(null)
  const { previewDocument, customTheme, refreshCustomTheme, setCustomTheme } = theme
  const tokens = useMemo(() => document[mode].tokens ?? {}, [document, mode])
  const colors = useResolvedColors(`${mode}:${JSON.stringify(document[mode])}`)

  // The whole app shows the theme as it's edited, in the mode being edited.
  useEffect(() => previewDocument(document, mode), [document, mode, previewDocument])
  useEffect(() => () => previewDocument(null), [previewDocument])

  const problems = documentProblems(document)
  const nameProblem = !name.trim() ? 'Give the theme a name.' : [...name.trim()].length > LIMITS.name ? `Keep the name to ${LIMITS.name} characters.` : null
  const failing = CONTRAST_PAIRS.map(([fg, bg, floor]) =>
    colors[fg] && colors[bg] ? { fg, bg, floor, ratio: contrastRatio(colors[fg], colors[bg]) } : null,
  ).filter((pair): pair is { fg: string; bg: string; floor: number; ratio: number } => Boolean(pair && pair.ratio < pair.floor))

  const setToken = (token: string, value: string | undefined) =>
    setDocument((current) => {
      const next = { ...(current[mode].tokens ?? {}) }
      if (value === undefined) delete next[token]
      else next[token] = value
      return { ...current, [mode]: { ...current[mode], tokens: next } }
    })
  const setBase = (base: string) => setDocument((current) => ({ ...current, [mode]: { ...current[mode], base } }))

  const save = async () => {
    setSaving(true)
    setError(null)
    try {
      const saved = start.id
        ? await callOp('theme_update', { id: start.id, expectedVersion: start.version, name: name.trim(), document }, { workspaceId })
        : await callOp('theme_create', { name: name.trim(), document }, { workspaceId })
      const cached = { id: saved.id, workspaceId, version: saved.version, name: saved.name, document: saved.document }
      if (!start.id) setCustomTheme(cached)
      else if (customTheme?.id === saved.id) refreshCustomTheme(cached)
      await invalidateOps(queryClient, ['theme_list', 'theme_get'], workspaceId)
      showToast({ title: start.id ? 'Theme saved' : `Using ${saved.name}` })
      onDone()
    } catch (reason) {
      setError(isVersionConflict(reason) ? 'This theme changed somewhere else since you opened it. Close the editor and open it again.' : describeError(reason).message)
    } finally {
      setSaving(false)
    }
  }

  return (
    <div className="aw-theme-editor">
      <Section
        actions={
          <SegmentedControl
            label="Mode to edit"
            onChange={setMode}
            options={[
              { value: 'light', label: 'Light' },
              { value: 'dark', label: 'Dark' },
            ]}
            value={mode}
          />
        }
        description="A theme sets light and dark, each starting from a built-in theme. Change any token; the app shows it as you go."
        title={start.id ? `Edit ${start.name}` : 'New theme'}
      >
        <TextField error={nameProblem ?? undefined} label="Name" maxLength={LIMITS.name} onChange={(event) => setName(event.target.value)} value={name} />
        <SelectField
          error={problems[`${mode}.base`]}
          label={`Starts from (${mode})`}
          onChange={(event) => setBase(event.target.value)}
          value={document[mode].base}
        >
          {THEME_PRESETS.filter((preset) => preset.scheme === mode).map((preset) => (
            <option key={preset.id} value={preset.id}>
              {preset.name}
            </option>
          ))}
        </SelectField>
      </Section>

      {failing.length ? (
        <div className="aw-contrast">
          <Banner tone="warning" title={`${countLabel(failing.length, 'colour')} ${failing.length === 1 ? 'is' : 'are'} hard to read in ${mode}.`}>
            Fix nudges the text colour until it passes.
          </Banner>
          <ul>
            {failing.slice(0, 5).map((pair) => (
              <li key={`${pair.fg}-${pair.bg}`}>
                <span>
                  {pair.fg.replace(/-/g, ' ')} on {pair.bg.replace(/-/g, ' ')}: <b>{pair.ratio.toFixed(1)}:1</b>, needs {pair.floor}:1
                </span>
                <Button onClick={() => setToken(pair.fg, fixContrast(colors[pair.fg], colors[pair.bg], pair.floor))} size="sm">
                  Fix
                </Button>
              </li>
            ))}
          </ul>
        </div>
      ) : null}

      {TOKEN_GROUPS.map((group) => {
        const set = group.tokens.filter((token) => tokens[token] !== undefined).length
        return (
          <details className="aw-token-group" key={group.id} open={OPEN_GROUPS.has(group.id)}>
            <summary>
              {group.label}
              {set ? <span className="aw-token-group__count">{countLabel(set, 'change')}</span> : null}
            </summary>
            <div className="aw-token-group__rows">
              {group.tokens.map((token) => (
                <TokenRow
                  colors={colors}
                  group={group}
                  key={token}
                  mode={mode}
                  onChange={(value) => setToken(token, value)}
                  token={token}
                  value={tokens[token]}
                />
              ))}
            </div>
          </details>
        )
      })}

      <Section
        description="CSS for both modes, on the part names below. It can use tokens and any selector, but can't load anything (no url(), @import or fonts from elsewhere)."
        title="CSS"
      >
        <TextArea
          className="aw-theme-editor__css"
          error={document.css ? (cssProblem(document.css) ?? undefined) : undefined}
          label="Custom CSS"
          onChange={(event) => setDocument((current) => ({ ...current, css: event.target.value }))}
          placeholder={'.aw-card {\n  border-radius: var(--radius-lg);\n}'}
          rows={10}
          spellCheck={false}
          value={document.css ?? ''}
        />
        <details className="aw-token-group">
          <summary>Part names</summary>
          <dl className="aw-theme-parts">
            {PARTS.map((part) => (
              <div key={part.class}>
                <dt>
                  <code>.{part.class}</code>
                </dt>
                <dd>{part.label}</dd>
              </div>
            ))}
          </dl>
        </details>
      </Section>

      {error ? <Banner tone="danger">{error}</Banner> : null}
      <div className="aw-theme-editor__actions">
        <Button disabled={saving || Boolean(nameProblem) || Object.keys(problems).length > 0} onClick={() => void save()} variant="primary">
          {start.id ? 'Save' : 'Save and use'}
        </Button>
        <Button onClick={onDone} variant="ghost">
          Cancel
        </Button>
      </div>
    </div>
  )
}
