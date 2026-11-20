import { cellText } from './outsideDataModel'

/** A stored answer of an outside data query as a table, scrolling sideways when it's wide. */
export function AnswerTable({ answer, most }: { answer: { columns: string[]; rows: unknown[] }; most?: number }) {
  const rows = most === undefined ? answer.rows : answer.rows.slice(0, most)
  if (!answer.columns.length) return <p className="aw-outside__quiet">No columns.</p>
  // A column of numbers lines up on the right, its heading too.
  const numeric = answer.columns.map((_, at) => {
    const cells = rows.map((row) => (Array.isArray(row) ? row[at] : null)).filter((cell) => cell !== null && cell !== undefined)
    return cells.length > 0 && cells.every((cell) => typeof cell === 'number')
  })
  return (
    <div className="aw-outside__table" role="region" aria-label="Answer" tabIndex={0}>
      <table>
        <thead>
          <tr>
            {answer.columns.map((column, index) => (
              <th className={numeric[index] ? 'aw-outside__num' : undefined} key={`${index}:${column}`} scope="col">
                {column}
              </th>
            ))}
          </tr>
        </thead>
        <tbody>
          {rows.length === 0 ? (
            <tr>
              <td colSpan={answer.columns.length}>No rows.</td>
            </tr>
          ) : (
            rows.map((row, index) => (
              <tr key={index}>
                {(Array.isArray(row) ? row : []).map((cell, at) => (
                  <td className={typeof cell === 'number' ? 'aw-outside__num' : undefined} key={at}>
                    {cellText(cell)}
                  </td>
                ))}
              </tr>
            ))
          )}
        </tbody>
      </table>
    </div>
  )
}

