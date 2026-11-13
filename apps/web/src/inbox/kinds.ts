import type { IconName } from '../ui'

/** The icon each notification kind shows on its row and preview. */
export const KIND_ICON: Record<string, IconName> = {
  mention: 'at-sign',
  comment_mention: 'at-sign',
  conversation_message: 'message',
  reminder: 'clock',
  scheduled_send_failed: 'circle-alert',
  assigned: 'work',
  approval_requested: 'shield-check',
  question_asked: 'circle-help',
  run_waiting: 'clock',
  run_completed: 'circle-check',
  run_failed: 'circle-alert',
  checks_failed: 'triangle-alert',
  review_requested: 'pull-request',
  task_changed: 'work',
  capacity_reset_soon: 'clock',
  capacity_high_usage: 'activity',
  capacity_limit_reached: 'circle-alert',
  capacity_reset_credit: 'retry',
}

/** Kinds that report something going wrong, drawn in the danger colour. */
export const FAILURE_KINDS = new Set(['scheduled_send_failed', 'run_failed', 'checks_failed', 'capacity_limit_reached'])
