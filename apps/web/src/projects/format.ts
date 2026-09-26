/** "owner" → "Owner", for roles shown to people. */
export function roleLabel(role: string | null | undefined) {
  if (!role) return 'Member'
  return role.charAt(0).toUpperCase() + role.slice(1)
}

/** Turns a server value like "not_configured" into "Not configured". */
export function humanize(value: string) {
  const words = value.replaceAll('_', ' ')
  return words.charAt(0).toUpperCase() + words.slice(1)
}
