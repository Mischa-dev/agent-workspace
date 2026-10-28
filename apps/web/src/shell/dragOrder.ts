/**
 * A list in its new order after one item is dropped before or after another,
 * or null when nothing moved (dropped on itself, or back where it was).
 */
export function orderAfterDrop<T extends { id: string }>(items: T[], draggedId: string, target: { id: string; after: boolean }): T[] | null {
  const dragged = items.find((item) => item.id === draggedId)
  if (!dragged || target.id === draggedId) return null
  const order = items.filter((item) => item.id !== draggedId)
  const at = order.findIndex((item) => item.id === target.id)
  if (at < 0) return null
  order.splice(at + (target.after ? 1 : 0), 0, dragged)
  return order.every((item, index) => item.id === items[index].id) ? null : order
}
