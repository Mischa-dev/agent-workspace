/** "Mischa" from "Mischa Nelson": Home talks to you, not to your account. */
export function firstName(displayName: string) {
  return displayName.trim().split(/\s+/)[0] || displayName
}

/** "Welcome" before you've opened anything, then a greeting for the time of day. */
export function homeGreeting(name: string, firstVisit: boolean, now = new Date()) {
  if (firstVisit) return `Welcome, ${name}`
  const hour = now.getHours()
  const part = hour >= 5 && hour < 12 ? 'morning' : hour >= 12 && hour < 18 ? 'afternoon' : 'evening'
  return `Good ${part}, ${name}`
}
