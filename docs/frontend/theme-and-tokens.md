# Theme and token contract

## Preference model

`ThemeProvider` owns presentation preferences only:

- theme: `system`, `dark`, or `light`
- density: `compact`, `comfortable`, or `spacious`
- custom theme version 1: validated overrides for `accent`, `human`,
  `externalAgent`, and `worker`

The local key is `agent-workspace:presentation:v1`. Parsing is fail-closed:
unknown enums, malformed JSON, unknown properties, and unsafe color strings are
dropped. Credentials, sessions, Actor records, Project records, capabilities,
drafts, or server responses must never be stored in this payload.

## Resolution

The provider applies `data-theme` and `data-density` to the document root and
sets `color-scheme`. System mode follows `prefers-color-scheme` without changing
the saved preference. Custom roles become bounded CSS custom properties; all
missing or invalid roles retain the selected theme defaults.

## Token layers

1. Primitive OKLCH values define neutral and chromatic ramps.
2. Semantic roles define surfaces, text, borders, actions, statuses, identity,
   focus, and overlays.
3. Component rules consume semantic roles only.

Spacing, radius, elevation, typography, density, motion, layout, and z-index are
also tokens. Features should not introduce local color ramps or arbitrary layer
numbers.

## Accessibility

- Text and state retain explicit labels; color is supplementary.
- Forced-colors mode preserves visible boundaries and focus.
- Reduced-motion mode removes nonessential animation.
- Density never overrides the 44 px touch-target floor at phone widths.
- Theme validation is presentation-only and cannot alter server authorization.
