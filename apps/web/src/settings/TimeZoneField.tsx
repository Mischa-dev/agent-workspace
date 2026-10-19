import { useMemo } from 'react'

import { SelectField, TextField } from '../ui'
import { deviceTimeZone, knownTimeZones } from './timeZones'

/**
 * The one way settings ask for a time zone: a list of real zones (type to jump
 * to one), with this device's first, so nobody has to spell "America/New_York".
 * Falls back to a text field on browsers that can't list them.
 */
export function TimeZoneField({
  disabled,
  hint,
  id,
  label = 'Time zone',
  onChange,
  value,
}: {
  disabled?: boolean
  hint?: string
  id?: string
  label?: string
  onChange: (timeZone: string) => void
  value: string
}) {
  const here = deviceTimeZone()
  const zones = useMemo(() => {
    const known = knownTimeZones()
    if (!known) return null
    const rest = known.filter((zone) => zone !== here && zone !== value)
    return [...new Set([here, ...(value && value !== here ? [value] : []), ...rest])]
  }, [here, value])

  if (!zones) {
    return (
      <TextField
        disabled={disabled}
        hint={hint ?? 'Such as Europe/Berlin.'}
        id={id}
        label={label}
        maxLength={64}
        onChange={(event) => onChange(event.target.value)}
        value={value}
      />
    )
  }
  return (
    <SelectField disabled={disabled} hint={hint} id={id} label={label} onChange={(event) => onChange(event.target.value)} value={value || here}>
      {zones.map((zone) => (
        <option key={zone} value={zone}>
          {zone.replaceAll('_', ' ')}
          {zone === here ? ' (this device)' : ''}
        </option>
      ))}
    </SelectField>
  )
}
