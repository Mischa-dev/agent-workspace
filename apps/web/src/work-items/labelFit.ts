/** The most chips, in order, that fit `available` pixels, leaving room for "+N" when some are left out. */
export function chipsThatFit(widths: number[], available: number, gap: number, moreWidth: number, limit = widths.length) {
  let best = 0
  let used = 0
  for (let count = 1; count <= Math.min(limit, widths.length); count += 1) {
    used += (count > 1 ? gap : 0) + widths[count - 1]
    const more = count < widths.length ? gap + moreWidth : 0
    // Half a pixel of slack for subpixel widths.
    if (used + more <= available + 0.5) best = count
  }
  return best
}
