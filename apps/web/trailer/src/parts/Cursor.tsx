import { clamp, ease, mix, progress } from '../motion'

export type Waypoint = { t: number; x: number; y: number }

/** Where the pointer is at time t: it glides between waypoints on a gentle arc. */
export function pointerAt(path: Waypoint[], t: number) {
  if (t <= path[0].t) return { x: path[0].x, y: path[0].y }
  for (let i = 0; i < path.length - 1; i += 1) {
    const a = path[i]
    const b = path[i + 1]
    if (t <= b.t) {
      const k = ease.inOut(clamp((t - a.t) / Math.max(0.001, b.t - a.t)))
      // Bow the path a little to one side, as a hand does.
      const dx = b.x - a.x
      const dy = b.y - a.y
      const bow = Math.sin(Math.PI * k) * 0.08
      return { x: mix(a.x, b.x, k) - dy * bow, y: mix(a.y, b.y, k) + dx * bow }
    }
  }
  const last = path[path.length - 1]
  return { x: last.x, y: last.y }
}

/** Whether a click is being pressed at t, for buttons that show their pressed state. */
export function pressed(clicks: number[], t: number) {
  return clicks.some((click) => t >= click && t < click + 0.16)
}

export function Cursor({
  clicks = [],
  hideAt,
  path,
  scale = 1.35,
  showAt,
  t,
}: {
  clicks?: number[]
  hideAt?: number
  path: Waypoint[]
  scale?: number
  showAt?: number
  t: number
}) {
  const start = showAt ?? path[0].t
  if (t < start - 0.3) return null
  const fadeIn = progress(t, start - 0.3, 0.3)
  const fadeOut = hideAt === undefined ? 0 : progress(t, hideAt, 0.3, ease.inOut)
  if (fadeOut >= 1) return null
  const { x, y } = pointerAt(path, t)
  const press = clicks.find((click) => t >= click && t < click + 0.6)
  const down = press !== undefined ? Math.sin(clamp((t - press) / 0.22) * Math.PI) : 0
  const ring = press !== undefined ? clamp((t - press) / 0.55) : 0
  return (
    <div className="tr-cursor" style={{ left: x, top: y, opacity: fadeIn * (1 - fadeOut) }}>
      {press !== undefined ? (
        <span
          className="tr-cursor__ring"
          style={{ opacity: (1 - ring) * 0.9, transform: `scale(${mix(0.35, 1.25, ease.out(ring))})` }}
        />
      ) : null}
      <svg
        height={26 * scale}
        style={{ transform: `scale(${1 - down * 0.14})`, transformOrigin: '3px 2px' }}
        viewBox="0 0 18 26"
        width={18 * scale}
      >
        <path
          d="M1 1 L1 20.5 L5.9 16.2 L9.4 24.2 L12.6 22.8 L9.2 15 L15.8 15 Z"
          fill="white"
          stroke="var(--tooltip-bg)"
          strokeLinejoin="round"
          strokeWidth="1.5"
        />
      </svg>
    </div>
  )
}
