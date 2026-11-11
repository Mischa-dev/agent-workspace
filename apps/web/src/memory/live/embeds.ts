/**
 * The embeds an open page editor shows (a collection's view, say). The
 * editor keeps an element for each; the page draws each one's live view into
 * its element through a portal, so the views share the app's data and
 * routing.
 */
export type EmbedSlot = { element: HTMLElement; href: string; label: string; key: string }

export class EmbedSlots {
  private slots = new Map<HTMLElement, EmbedSlot>()
  private listeners = new Set<() => void>()
  private snapshot: EmbedSlot[] = []
  private made = 0

  subscribe = (listener: () => void) => {
    this.listeners.add(listener)
    return () => {
      this.listeners.delete(listener)
    }
  }

  getSnapshot = () => this.snapshot

  set(slot: Omit<EmbedSlot, 'key'>) {
    const current = this.slots.get(slot.element)
    if (current && current.href === slot.href && current.label === slot.label) return
    this.made += 1
    this.slots.set(slot.element, { ...slot, key: current?.key ?? `embed-${this.made}` })
    this.changed()
  }

  remove(element: HTMLElement) {
    if (this.slots.delete(element)) this.changed()
  }

  private changed() {
    this.snapshot = [...this.slots.values()]
    for (const listener of this.listeners) listener()
  }
}
