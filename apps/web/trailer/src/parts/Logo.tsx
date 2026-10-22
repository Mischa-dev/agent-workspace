import { clamp, ease, mix, progress } from '../motion'

/*
 * The app's mark (public/icon.svg): a round person and a rounded-square agent,
 * side by side, which is also the design system's first identity rule. The
 * two shapes build in one after the other, in the brand and agent colors.
 */
export function Mark({
  at,
  size,
  t,
  tile = false,
}: {
  at: number
  size: number
  t: number
  tile?: boolean
}) {
  const circle = progress(t, at, 0.7)
  const square = progress(t, at + 0.28, 0.75)
  const tileIn = tile ? progress(t, at - 0.2, 0.6) : 0
  const ground = tile ? 'var(--rail-bg)' : 'var(--brand-deep)'
  return (
    <svg height={size} viewBox="0 0 512 512" width={size}>
      {tile ? <rect height="512" rx="112" style={{ fill: ground, opacity: tileIn }} width="512" /> : null}
      <circle cx="193" cy="193" r={87 * ease.out(clamp(circle))} style={{ fill: 'var(--brand)', opacity: clamp(circle * 2) }} />
      <g
        style={{
          opacity: clamp(square * 2),
          transform: `translate(${mix(70, 0, square)}px, ${mix(70, 0, square)}px) scale(${mix(0.55, 1, square)})`,
          transformOrigin: '319px 319px',
        }}
      >
        <rect height="219" rx="71.2" style={{ fill: ground }} width="219" x="209.5" y="209.5" />
        <rect height="174" rx="48.7" style={{ fill: 'var(--agent-working)' }} width="174" x="232" y="232" />
      </g>
    </svg>
  )
}
