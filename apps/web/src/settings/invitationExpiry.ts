/**
 * How long an invitation link works, the same choices everywhere links are
 * made. Project invitations stop at 7 days (the server's limit for them).
 */
export const INVITATION_EXPIRY_OPTIONS = [
  { hours: 24, label: '1 day' },
  { hours: 168, label: '7 days' },
  { hours: 720, label: '30 days' },
]

export const DEFAULT_INVITATION_HOURS = 168

export const PROJECT_INVITATION_MAX_HOURS = 168
