/** "Design review" → "DR", "Trident" → "TRI": a starting point the person can change. */
export function suggestKey(name: string) {
  const words = name
    .toLocaleUpperCase()
    .replace(/[^A-Z0-9]+/g, ' ')
    .trim()
    .split(' ')
    .filter(Boolean)
  if (!words.length) return ''
  const initials = words.map((word) => word[0]).join('')
  const key = (words.length > 1 && initials.length >= 2 ? initials : words[0]).replace(/^[0-9]+/, '')
  return key.slice(0, words.length > 1 ? 4 : 3)
}

export const projectKeyPattern = /^[A-Z][A-Z0-9]{1,9}$/
