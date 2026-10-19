/** Every time zone this browser knows, or null when it can't list them. */
export function knownTimeZones(): string[] | null {
  try {
    const list = (Intl as { supportedValuesOf?: (key: 'timeZone') => string[] }).supportedValuesOf?.('timeZone')
    return list?.length ? list : null
  } catch {
    return null
  }
}

/** This device's time zone, such as Europe/Berlin. */
export function deviceTimeZone() {
  try {
    return Intl.DateTimeFormat().resolvedOptions().timeZone || 'UTC'
  } catch {
    return 'UTC'
  }
}
