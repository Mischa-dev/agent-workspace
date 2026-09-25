// Applies the saved theme before the app loads, so the first paint is already in the person's colors.
// Mirrors src/theme/preferences.ts and src/theme/slots.ts; keep the three in step.
;(function () {
  var presets = ['indigo', 'midnight', 'daylight', 'evergreen', 'graphite']
  var derived = {
    'rail-tile': 'color-mix(in oklch, var(--rail-fg) 12%, var(--rail-bg))',
    'sidebar-hover': 'color-mix(in oklch, var(--sidebar-fg-strong) 9%, var(--sidebar-bg))',
    'sidebar-border': 'color-mix(in oklch, var(--sidebar-fg-strong) 12%, var(--sidebar-bg))',
    'sidebar-fg-muted': 'color-mix(in oklch, var(--sidebar-fg) 82%, var(--sidebar-bg))',
    'content-hover': 'color-mix(in oklch, var(--content-fg) 3%, var(--content-bg))',
    'content-border': 'color-mix(in oklch, var(--content-fg) 9%, var(--content-bg))',
    'surface-sunken': 'color-mix(in oklch, var(--content-fg) 4%, var(--content-bg))',
    'accent-hover': 'color-mix(in oklch, var(--accent) 86%, black)',
    'accent-soft': 'color-mix(in oklch, var(--accent) 10%, var(--content-bg))',
    'focus-ring': 'var(--accent)',
    'mention-fg': 'color-mix(in oklch, var(--accent) 88%, var(--content-fg))',
    'mention-bg': 'color-mix(in oklch, var(--accent) 12%, var(--content-bg))',
    'agent-soft': 'color-mix(in oklch, var(--agent) 12%, var(--content-bg))',
  }
  var root = document.documentElement
  var saved = {}
  try {
    var raw = window.localStorage.getItem('agent-workspace:appearance:v2')
    if (raw) saved = JSON.parse(raw) || {}
    else {
      var legacy = JSON.parse(window.localStorage.getItem('agent-workspace:appearance:v1') || '{}') || {}
      saved = { theme: legacy.theme === 'dark' ? 'midnight' : 'indigo' }
    }
  } catch (error) {
    saved = {}
  }
  var theme = saved.theme
  if (theme === 'system') {
    theme = window.matchMedia && window.matchMedia('(prefers-color-scheme: dark)').matches ? 'midnight' : 'indigo'
  }
  root.setAttribute('data-theme', presets.indexOf(theme) >= 0 ? theme : 'indigo')
  root.setAttribute('data-density', saved.density === 'compact' ? 'compact' : 'comfortable')
  root.setAttribute('data-text-size', saved.textSize === 'small' || saved.textSize === 'large' ? saved.textSize : 'default')
  var overrides = saved.overrides && typeof saved.overrides === 'object' ? saved.overrides : {}
  var changed = {}
  Object.keys(overrides).forEach(function (slot) {
    var color = overrides[slot]
    if (/^[a-z-]+$/.test(slot) && typeof color === 'string' && /^#[0-9a-f]{6}$/i.test(color)) {
      root.style.setProperty('--' + slot, color)
      changed[slot] = true
    }
  })
  Object.keys(derived).forEach(function (slot) {
    if (changed[slot]) return
    var bases = derived[slot].match(/--[a-z-]+/g) || []
    for (var i = 0; i < bases.length; i += 1) {
      if (changed[bases[i].slice(2)]) {
        root.style.setProperty('--' + slot, derived[slot])
        return
      }
    }
  })
})()
