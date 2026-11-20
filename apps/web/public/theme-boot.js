// Applies the saved theme before the app loads, so the first paint is already in the person's colors.
// Mirrors src/theme/preferences.ts, src/theme/slots.ts and src/theme/customTheme.ts; keep them in step.
;(function () {
  var presets = ['quiet', 'quiet-dark', 'indigo', 'midnight', 'daylight', 'evergreen', 'graphite']
  // The rest live in a sheet under /themes/, loaded only for people who use one (presetSheet in slots.ts).
  var paperFrame = ['paper', 'paper-dark', 'frame', 'frame-dark']
  var classic = [
    'nord', 'dracula', 'gruvbox-dark', 'solarized-dark', 'catppuccin-frappe', 'catppuccin-macchiato',
    'catppuccin-mocha', 'tokyo-night', 'one-dark', 'rose-pine', 'github-dark', 'monokai',
    'everforest-dark', 'kanagawa',
    'gruvbox-light', 'solarized-light', 'catppuccin-latte', 'rose-pine-dawn', 'github-light',
    'everforest-light', 'one-light',
  ]
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
      saved = { theme: legacy.theme === 'dark' ? 'midnight' : legacy.theme === 'light' ? 'indigo' : 'system' }
    }
  } catch (error) {
    saved = {}
  }
  var dark = !!(window.matchMedia && window.matchMedia('(prefers-color-scheme: dark)').matches)
  var known = function (id) {
    return presets.indexOf(id) >= 0 || classic.indexOf(id) >= 0 || paperFrame.indexOf(id) >= 0
  }
  var theme = saved.theme
  // Until someone picks a theme, it's Quiet, light or dark as the device is (DEFAULT_APPEARANCE in preferences.ts).
  if (!known(theme)) theme = 'system'
  if (theme === 'system') theme = dark ? 'quiet-dark' : 'quiet'
  var properties = {}
  var css = ''
  // A custom theme, cached by the app (customTheme.ts): its base, tokens and CSS.
  var custom = null
  if (saved.customTheme && typeof saved.customTheme.id === 'string') {
    try {
      custom = JSON.parse(window.localStorage.getItem('agent-workspace:theme-document:v1') || 'null')
    } catch (error) {
      custom = null
    }
    if (!custom || custom.id !== saved.customTheme.id || !custom.document) custom = null
  }
  if (custom) {
    var mode = saved.mode === 'light' || saved.mode === 'dark' ? saved.mode : dark ? 'dark' : 'light'
    var half = custom.document[mode] || {}
    theme = known(half.base) ? half.base : mode === 'dark' ? 'quiet-dark' : 'quiet'
    var tokens = half.tokens && typeof half.tokens === 'object' ? half.tokens : {}
    Object.keys(tokens).forEach(function (name) {
      var value = tokens[name]
      if (/^[a-z0-9-]+$/.test(name) && typeof value === 'string' && value.trim() && !/[;{}<>\\@!\u0000-\u001f]|\/\*|url\(/i.test(value)) {
        properties[name] = value.trim()
      }
    })
    var text = typeof custom.document.css === 'string' ? custom.document.css : ''
    // Comments are checked out, as the editor and the server check them.
    if (!/[\\<]|url\(|@import|@font-face|@namespace|image-set\(|image\(|src\(/i.test(text.replace(/\/\*[\s\S]*?(\*\/|$)/g, ''))) css = text
  } else {
    var overrides = saved.overrides && typeof saved.overrides === 'object' ? saved.overrides : {}
    Object.keys(overrides).forEach(function (slot) {
      var color = overrides[slot]
      if (/^[a-z-]+$/.test(slot) && typeof color === 'string' && /^#[0-9a-f]{6}$/i.test(color)) properties[slot] = color
    })
  }
  var sheet = classic.indexOf(theme) >= 0 ? 'classic' : paperFrame.indexOf(theme) >= 0 ? 'paper-frame' : ''
  if (sheet) {
    // Written by the parser, so the stylesheet holds the first paint and nothing flashes in Quiet.
    document.write('<link rel="stylesheet" href="/themes/' + sheet + '.css" data-theme-sheet="' + sheet + '">')
  }
  root.setAttribute('data-theme', known(theme) ? theme : 'quiet')
  root.setAttribute('data-density', saved.density === 'compact' ? 'compact' : 'comfortable')
  root.setAttribute('data-text-size', saved.textSize === 'small' || saved.textSize === 'large' ? saved.textSize : 'default')
  // The sidebar and side panel as wide as they were dragged (SIDEBAR_WIDTHS and PANEL_WIDTHS in preferences.ts).
  var widths = { '--sidebar-width': [saved.sidebarWidth, 200, 480], '--panel-width': [saved.panelWidth, 320, 800] }
  Object.keys(widths).forEach(function (name) {
    var width = widths[name]
    if (typeof width[0] === 'number' && isFinite(width[0])) {
      root.style.setProperty(name, Math.round(Math.min(width[2], Math.max(width[1], width[0]))) + 'px')
    }
  })
  var set = []
  Object.keys(properties).forEach(function (name) {
    root.style.setProperty('--' + name, properties[name])
    set.push('--' + name)
  })
  Object.keys(derived).forEach(function (slot) {
    if (properties[slot]) return
    var bases = derived[slot].match(/--[a-z-]+/g) || []
    for (var i = 0; i < bases.length; i += 1) {
      if (properties[bases[i].slice(2)]) {
        root.style.setProperty('--' + slot, derived[slot])
        set.push('--' + slot)
        return
      }
    }
  })
  // ThemeProvider takes these over (and moves the CSS after the app's stylesheets).
  root.setAttribute('data-theme-properties', set.join(' '))
  if (css) {
    var style = document.createElement('style')
    style.id = 'aw-theme-css'
    style.textContent = css
    document.head.appendChild(style)
  }
})()
