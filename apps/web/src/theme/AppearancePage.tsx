import { useEffect, useMemo, useRef, useState, type CSSProperties } from 'react'

import {
  ActorTag,
  Avatar,
  Badge,
  Banner,
  Button,
  Dots,
  Icon,
  IconButton,
  PageHeader,
  Section,
  SegmentedControl,
  TextField,
} from '../ui'
import {
  CONTRAST_PAIRS,
  SLOT_GROUPS,
  SLOT_LABELS,
  THEME_PRESETS,
  ThemeCodeError,
  contrastRatio,
  decodeTheme,
  encodeTheme,
  fixContrast,
  toHex,
  type Rgb,
} from './slots'
import { useTheme } from './themeContext'

/** Read what every slot resolves to on <html>, as sRGB, for swatches and contrast. */
function useResolvedColors(revision: string) {
  const [colors, setColors] = useState<Record<string, Rgb>>({})
  useEffect(() => {
    const canvas = document.createElement('canvas').getContext('2d', { willReadFrequently: true })
    if (!canvas) return undefined
    // Read after the theme provider has applied this revision to <html>.
    const frame = window.requestAnimationFrame(() => {
    const probe = document.createElement('span')
    probe.style.position = 'absolute'
    probe.style.width = '0'
    probe.style.height = '0'
    document.body.appendChild(probe)
    const next: Record<string, Rgb> = {}
    for (const group of SLOT_GROUPS) {
      for (const [slot] of group.slots) {
        probe.style.color = `var(--${slot})`
        canvas.clearRect(0, 0, 1, 1)
        canvas.fillStyle = '#000'
        canvas.fillStyle = getComputedStyle(probe).color
        canvas.fillRect(0, 0, 1, 1)
        const [r, g, b] = canvas.getImageData(0, 0, 1, 1).data
        next[slot] = [r, g, b]
      }
    }
    probe.remove()
    setColors(next)
    })
    return () => window.cancelAnimationFrame(frame)
  }, [revision])
  return colors
}

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

/** A small, real rendering of the shell so the sidebar and rail colors show on this page too. */
function ThemePreview() {
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
          <Avatar name="Mischa Nelson" size={28} />
          <div>
            <div className="aw-msg__meta">
              <span className="aw-msg__author">Mischa Nelson</span>
              <span className="aw-msg__time">9:41 AM</span>
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
              <span className="aw-msg__time">9:44 AM</span>
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

/** Account › Appearance in settings. */
export default function AppearancePage() {
  const theme = useTheme()
  const [mode, setMode] = useState<'main' | 'all'>('main')
  const [pasted, setPasted] = useState('')
  const [pasteError, setPasteError] = useState<string | null>(null)
  const [copied, setCopied] = useState(false)
  const copiedTimer = useRef<number | undefined>(undefined)
  const revision = `${theme.preset}:${JSON.stringify(theme.overrides)}`
  const colors = useResolvedColors(revision)
  const code = encodeTheme(theme.preset, theme.overrides)
  useEffect(() => () => window.clearTimeout(copiedTimer.current), [])

  const failing = useMemo(
    () =>
      CONTRAST_PAIRS.map(([fg, bg, floor]) =>
        colors[fg] && colors[bg] ? { fg, bg, floor, ratio: contrastRatio(colors[fg], colors[bg]) } : null,
      ).filter((pair): pair is { fg: string; bg: string; floor: number; ratio: number } => Boolean(pair && pair.ratio < pair.floor)),
    [colors],
  )
  const failingSlots = new Set(failing.map((pair) => pair.fg))

  const setSlot = (slot: string, color: string) => theme.setOverrides({ ...theme.overrides, [slot]: color.toLowerCase() })
  const resetSlot = (slot: string) => {
    const next = { ...theme.overrides }
    delete next[slot]
    theme.setOverrides(next)
  }
  const copy = () => {
    const done = () => {
      setCopied(true)
      window.clearTimeout(copiedTimer.current)
      copiedTimer.current = window.setTimeout(() => setCopied(false), 1600)
    }
    void navigator.clipboard?.writeText(code).then(done, () => document.getElementById('theme-code')?.focus())
  }
  const apply = () => {
    try {
      const decoded = decodeTheme(pasted)
      theme.setTheme(decoded.preset)
      theme.setOverrides(decoded.overrides)
      setPasted('')
      setPasteError(null)
    } catch (reason) {
      setPasteError(reason instanceof ThemeCodeError ? reason.message : 'That theme code didn’t work.')
    }
  }
  const presetName = THEME_PRESETS.find((preset) => preset.id === theme.preset)?.name ?? 'Indigo'

  return (
      <div className="aw-appearance">
        <PageHeader
          actions={
            <Button icon="undo" onClick={theme.resetAppearance} variant="ghost">
              Reset everything
            </Button>
          }
          icon="palette"
          title="Appearance"
        >
          Start from a preset, then change any color. Changes apply as you make them and follow you to every device you sign in on.
        </PageHeader>

        <div className="aw-appearance__layout">
          <div className="aw-appearance__controls">
            <Section title="Theme">
              <div aria-label="Theme" className="aw-presets" role="radiogroup">
                {THEME_PRESETS.map((preset) => (
                  <button
                    aria-checked={theme.theme === preset.id}
                    className="aw-preset"
                    key={preset.id}
                    onClick={() => theme.setTheme(preset.id)}
                    role="radio"
                    type="button"
                  >
                    <PresetMini id={preset.id} />
                    <span className="aw-preset__name">{preset.name}</span>
                  </button>
                ))}
                <button
                  aria-checked={theme.theme === 'system'}
                  className="aw-preset"
                  onClick={() => theme.setTheme('system')}
                  role="radio"
                  type="button"
                >
                  <span aria-hidden="true" className="aw-mini aw-mini--split">
                    <PresetMini id="indigo" />
                    <PresetMini id="midnight" />
                  </span>
                  <span className="aw-preset__name">Match system</span>
                </button>
              </div>
            </Section>

            <Section
              actions={
                <SegmentedControl
                  label="Which colors to show"
                  onChange={setMode}
                  options={[
                    { value: 'main', label: 'Main' },
                    { value: 'all', label: 'Every color' },
                  ]}
                  value={mode}
                />
              }
              description={`Based on ${presetName}. Changing a background also adjusts its hover and divider colors, unless you set those yourself.`}
              title="Colors"
            >
              {failing.length === 0 ? (
                Object.keys(colors).length ? <Banner tone="success">Every text color reads clearly on its background.</Banner> : null
              ) : (
                <div className="aw-contrast">
                  <Banner tone="warning" title={`${failing.length} ${failing.length === 1 ? 'color is' : 'colors are'} hard to read.`}>
                    Fix nudges the text color until it passes.
                  </Banner>
                  <ul>
                    {failing.slice(0, 5).map((pair) => (
                      <li key={`${pair.fg}-${pair.bg}`}>
                        <span>
                          {SLOT_LABELS[pair.fg]} on {(SLOT_LABELS[pair.bg] ?? pair.bg).toLocaleLowerCase()}: <b>{pair.ratio.toFixed(1)}:1</b>, needs{' '}
                          {pair.floor}:1
                        </span>
                        <Button onClick={() => setSlot(pair.fg, fixContrast(colors[pair.fg], colors[pair.bg], pair.floor))} size="sm">
                          Fix
                        </Button>
                      </li>
                    ))}
                  </ul>
                </div>
              )}
              {SLOT_GROUPS.map((group) => {
                const slots = group.slots.filter((entry) => mode === 'all' || entry[2])
                return (
                  <div className="aw-slots" key={group.label}>
                    <h3 className="aw-slots__title">{group.label}</h3>
                    {slots.map(([slot, label]) => {
                      const value = colors[slot] ? toHex(colors[slot]) : '#000000'
                      const set = Boolean(theme.overrides[slot])
                      return (
                        <div className={set ? 'aw-slot is-set' : 'aw-slot'} key={slot}>
                          <span className="aw-slot__swatch" style={{ background: `var(--${slot})` }}>
                            <input
                              aria-label={`${group.label} ${label.toLocaleLowerCase()}`}
                              onChange={(event) => setSlot(slot, event.target.value)}
                              type="color"
                              value={value}
                            />
                          </span>
                          <span className="aw-slot__name">{label}</span>
                          {failingSlots.has(slot) ? <Icon className="aw-slot__warn" label="Hard to read" name="triangle-alert" size={14} /> : null}
                          <code className="aw-slot__hex">{value}</code>
                          {set ? (
                            <IconButton icon="undo" label={`Reset ${label.toLocaleLowerCase()}`} onClick={() => resetSlot(slot)} size="sm" />
                          ) : (
                            <span className="aw-slot__spacer" />
                          )}
                        </div>
                      )
                    })}
                  </div>
                )
              })}
            </Section>

            <Section description="Share a theme as a short code. Paste one to try it." title="Share">
              <div className="aw-share">
                <TextField id="theme-code" label="Your theme code" mono onFocus={(event) => event.target.select()} readOnly value={code} />
                <Button icon={copied ? 'check' : 'copy'} onClick={copy}>
                  {copied ? 'Copied' : 'Copy'}
                </Button>
              </div>
              <div className="aw-share">
                <TextField
                  error={pasteError ?? undefined}
                  id="theme-code-paste"
                  label="Use a theme code"
                  mono
                  onChange={(event) => {
                    setPasted(event.target.value)
                    setPasteError(null)
                  }}
                  placeholder="AW1.…"
                  value={pasted}
                />
                <Button disabled={!pasted.trim()} onClick={apply}>
                  Apply
                </Button>
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
          </div>
          <div className="aw-appearance__preview">
            <p className="aw-appearance__label">Preview</p>
            <ThemePreview />
          </div>
        </div>
      </div>
  )
}
