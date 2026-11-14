import { useLayoutEffect, useRef, useState, type RefObject } from 'react'

import { Tooltip } from '../ui'
import { chipsThatFit } from './labelFit'
import type { WorkItemLabel } from './types'
import '../styles/work.css'

/**
 * Measures how many whole chips fit beside their siblings in the parent row,
 * and keeps that count as the row resizes. Chips that don't fit stay in the
 * list, out of the flow and invisible, so they can still be measured.
 */
function useFittedCount(enabled: boolean, list: RefObject<HTMLUListElement | null>, total: number, limit: number, key: string) {
  const [fitted, setFitted] = useState({ count: limit, room: true })
  useLayoutEffect(() => {
    const element = list.current
    const parent = element?.parentElement
    if (!enabled || !element || !parent) return undefined
    const measure = () => {
      const chips = [...element.querySelectorAll<HTMLElement>('[data-chip]')]
      const more = element.querySelector<HTMLElement>('[data-more]')
      const parentStyle = getComputedStyle(parent)
      const parentGap = parseFloat(parentStyle.columnGap) || 0
      const siblings = [...parent.children].filter((child) => child !== element)
      const besides = siblings.reduce((sum, child) => sum + child.getBoundingClientRect().width + parentGap, 0)
      const inner = parent.clientWidth - (parseFloat(parentStyle.paddingLeft) || 0) - (parseFloat(parentStyle.paddingRight) || 0)
      const gap = parseFloat(getComputedStyle(element).columnGap) || 0
      const widths = chips.map((chip) => chip.getBoundingClientRect().width)
      const moreWidth = more?.getBoundingClientRect().width ?? 0
      const count = chipsThatFit(widths, inner - besides, gap, moreWidth, limit)
      // Not even "+N" fits beside a crowded row: show nothing rather than half a chip.
      setFitted({ count, room: count > 0 || inner - besides + 0.5 >= moreWidth })
    }
    measure()
    if (typeof ResizeObserver === 'undefined') return undefined
    const observer = new ResizeObserver(measure)
    observer.observe(parent)
    for (const sibling of parent.children) if (sibling !== element) observer.observe(sibling)
    return () => observer.disconnect()
  }, [enabled, key, limit, list, total])
  return enabled ? { count: Math.min(fitted.count, limit), room: fitted.room } : { count: limit, room: true }
}

/**
 * A task's labels as small chips, read-only. Only the dot carries the
 * label's color, so the text keeps its contrast in both themes. With `fit`,
 * as many whole chips as fit beside the rest of the row show, then "+N" with
 * the others in a tooltip; `max` caps them either way.
 */
export function WorkItemLabels({ fit = false, labels, max }: { fit?: boolean; labels: WorkItemLabel[]; max?: number }) {
  const list = useRef<HTMLUListElement>(null)
  const limit = max === undefined ? labels.length : Math.min(max, labels.length)
  const { count: shownCount, room } = useFittedCount(fit, list, labels.length, limit, labels.map((label) => `${label.id}:${label.name}`).join('|'))
  if (labels.length === 0) return null
  const hidden = labels.slice(shownCount)
  const names = hidden.map((label) => label.name).join(', ')
  return (
    <ul aria-label="Labels" className={fit ? 'aw-labels is-fitted' : 'aw-labels'} ref={list}>
      {labels.map((label, index) =>
        index < shownCount || fit ? (
          <li aria-hidden={index >= shownCount || undefined} className={index < shownCount ? 'aw-label' : 'aw-label is-overflow'} data-chip="" key={label.id}>
            <span aria-hidden className="aw-label__dot" style={{ backgroundColor: label.color }} />
            <span className="aw-label__name">{label.name}</span>
          </li>
        ) : null,
      )}
      {hidden.length > 0 ? (
        room ? (
          <li aria-label={`${hidden.length} more: ${names}`} className="aw-label aw-label--more" data-more="">
            <Tooltip label={names}>
              <span>+{hidden.length}</span>
            </Tooltip>
          </li>
        ) : (
          // No room even for "+N": read out, not shown, and still measured so the row can grow back.
          <>
            <li className="visually-hidden">{names}</li>
            <li aria-hidden="true" className="aw-label aw-label--more is-overflow" data-more="">
              +{hidden.length}
            </li>
          </>
        )
      ) : fit ? (
        // Kept for measuring, so the count knows how wide "+N" would be.
        <li aria-hidden="true" className="aw-label aw-label--more is-overflow" data-more="">
          +{Math.max(1, labels.length - 1)}
        </li>
      ) : null}
    </ul>
  )
}
