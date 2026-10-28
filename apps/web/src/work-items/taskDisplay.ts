import type { TableColumn } from '../ui/pickers'
import type { WorkItemCard } from './types'

export const LIST_COLUMNS: TableColumn[] = [
  { id: 'select', header: 'Select', width: '36px', quietHeader: true },
  { id: 'priority', header: 'Priority', width: '32px', quietHeader: true },
  { id: 'key', header: 'Key', width: '76px', hideOnPhone: true },
  { id: 'status', header: 'Status', width: '32px', quietHeader: true },
  { id: 'title', header: 'Title', width: 'minmax(0, 1fr)' },
  { id: 'details', header: 'Details', width: 'minmax(0, min(240px, 25%))', hideOnPhone: true, quietHeader: true, align: 'end' },
  { id: 'due', header: 'Due', width: '84px', hideOnPhone: true },
  { id: 'assignee', header: 'Assignee', width: '40px', quietHeader: true, align: 'end' },
]

export function todayIso() {
  const now = new Date()
  return `${now.getFullYear()}-${String(now.getMonth() + 1).padStart(2, '0')}-${String(now.getDate()).padStart(2, '0')}`
}

export function isFinished(card: Pick<WorkItemCard, 'status'>) {
  return card.status.semanticCategory === 'completed' || card.status.semanticCategory === 'canceled'
}

export function isOverdue(card: Pick<WorkItemCard, 'dueDate' | 'status'>, today = todayIso()) {
  return Boolean(card.dueDate && card.dueDate < today && !isFinished(card))
}

