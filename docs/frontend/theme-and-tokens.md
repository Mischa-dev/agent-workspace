# Theme and token contract

## Preference model

`ThemeProvider` owns presentation preferences only:

- theme: a preset (`indigo`, `midnight`, `daylight`, `evergreen`, `graphite`)
  or `system` (Indigo in light mode, Midnight in dark mode)
- overrides: a map of theme slot to six-digit hex color
- density: `comfortable` or `compact`
- text size: `small`, `default` or `large`

The local key is `agent-workspace:appearance:v2`. A `v1` value is migrated on
read (dark becomes Midnight, anything else Indigo). Parsing fails closed:
unknown presets, slots and anything that isn't `#rrggbb` are dropped, so only
known slots and plain hex colors ever reach CSS. Credentials, sessions, actors,
projects, capabilities, drafts and server responses are never stored here.

## Resolution

The provider sets `data-theme`, `data-density` and `data-text-size` on
`<html>` and writes each override as an inline custom property. When a base slot
changes, the slots that follow it (`src/theme/slots.ts` `DERIVED_SLOTS`, for
example `sidebar-hover` from `sidebar-bg`) get a `color-mix()` formula unless the
person set them directly. `public/theme-boot.js` applies the same preferences
before the app loads so the first paint is already in the person's colors; keep
it in step with `preferences.ts` and `slots.ts`.

## Token layers

1. Presets: the five `[data-theme]` blocks in `src/styles/tokens.css`,
   generated from the design system's `tokens.json` (contrast-checked for every
   pair in every preset).
2. Regions: `--r-*` variables set by `data-region` on the rail, sidebar and
   content columns.
3. Components read slots and region variables only; no literal colors.

Every page now uses these tokens directly. The pre-redesign aliases
(`--color-canvas`, `--text`, `--panel` and so on) were removed in pass two, so
don't reintroduce them.

## Theme codes

`encodeTheme` / `decodeTheme` in `src/theme/slots.ts` read and write
`AW1.<base64url JSON {b: preset, o: overrides}>`. Decoding applies the same
fail-closed rules as stored preferences.

## Contrast

`CONTRAST_PAIRS` lists the pairs that carry meaning: text at 4.5:1, icons,
borders, focus rings and dots at 3:1. The Appearance page checks them on every
change, flags the slot and offers Fix, which walks the text color toward black
or white until the pair passes.
