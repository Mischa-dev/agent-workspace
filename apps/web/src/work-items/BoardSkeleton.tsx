const CARDS = [3, 2, 4, 1]
const WIDTHS = [72, 54, 81, 63]

/** The board while it loads: a few columns of cards, so the page doesn't jump when it arrives. */
export function BoardSkeleton({ label = 'Loading the board' }: { label?: string }) {
  return (
    <div aria-busy="true" aria-label={label} className="aw-skel aw-skel--shaped aw-tboard-skel" role="status">
      {CARDS.map((count, column) => (
        <div className="aw-tboard-skel__column" key={column}>
          <span className="aw-skel__bar is-strong" style={{ width: `${40 + column * 9}%` }} />
          {Array.from({ length: count }, (_, card) => (
            <div className="aw-tboard-skel__card" key={card}>
              <span className="aw-skel__bar" style={{ width: '28%' }} />
              <span className="aw-skel__bar is-strong" style={{ width: `${WIDTHS[(card + column) % WIDTHS.length]}%` }} />
              <span className="aw-skel__bar" style={{ width: '44%' }} />
            </div>
          ))}
        </div>
      ))}
    </div>
  )
}
