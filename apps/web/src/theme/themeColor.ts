import { toHex } from './slots'

let probe: CanvasRenderingContext2D | null | undefined

/** A CSS color as #rrggbb, which every browser accepts in theme-color; the input unchanged if it can't convert. */
function cssColorToHex(color: string) {
  if (/^#[0-9a-f]{6}$/i.test(color)) return color
  if (probe === undefined) {
    try {
      probe = document.createElement('canvas').getContext('2d', { willReadFrequently: true }) ?? null
    } catch {
      probe = null
    }
  }
  if (!probe) return color
  const sentinel = '#010203'
  probe.fillStyle = sentinel
  probe.fillStyle = color
  if (probe.fillStyle === sentinel) return color
  probe.clearRect(0, 0, 1, 1)
  probe.fillRect(0, 0, 1, 1)
  const [red = 0, green = 0, blue = 0] = probe.getImageData(0, 0, 1, 1).data
  return toHex([red, green, blue])
}

/**
 * Colors the phone status bar and browser toolbar to match the top of the app:
 * the conversation header in a conversation, the space bar everywhere else.
 * AppShell sets data-chrome; ThemeProvider calls this again when the theme changes.
 */
export function syncThemeColor() {
  const root = document.documentElement
  const meta = document.querySelector('meta[name="theme-color"]')
  if (!meta) return
  const slot = root.dataset.chrome === 'conversation' ? '--content-bg' : '--sidebar-bg'
  const color = getComputedStyle(root).getPropertyValue(slot).trim()
  if (color) meta.setAttribute('content', cssColorToHex(color))
}
