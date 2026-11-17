# Themes

Every part of the app's look is a theme. Quiet (light and dark) is the
default. The other built-in themes are Paper, Frame, Indigo, Midnight,
Daylight, Evergreen, Graphite and the classic palettes. Anyone can make
their own theme in Settings › Account › Appearance, or through the theme
tools.

## What a theme is

A theme is one document, stored in the workspace it was made in:

```json
{
  "name": "Harbour",
  "format": 1,
  "light": { "base": "quiet", "tokens": { "accent": "#1f6f8b", "radius-md": "4px" } },
  "dark": { "base": "quiet-dark", "tokens": { "accent": "#6fc3df" } },
  "css": ".aw-card { border-width: 2px; }"
}
```

- **`light` and `dark`** each start from a built-in theme of that scheme
  (`base`). They change any token (`tokens`). A token left out keeps the
  base's value. Which half shows follows the device, unless the person picks
  Light or Dark under Mode.
- **`css`** applies in both modes. It loads after the app's own styles.
- **`name`** is only in files. Export writes it and Import reads it.

`apps/web/src/theme/contract.json` is the contract, and it is versioned by
`format`. It lists:

- the tokens and their groups: colour, type, spacing, shape, depth and motion;
- the built-in themes;
- the published part names;
- the functions and at-rules the CSS may use;
- the size limits.

The server (`apps/server/src/themes.rs`) and the web
(`apps/web/src/theme/contract.ts`) run the same checks on every theme.

## Tokens

Token values are colours, lengths, font stacks, shadows, durations and
easings. A value may use:

- `calc()`, `color-mix()` and the colour functions;
- `var()` of another theme token.

A value can't contain `;`, braces, `<`, `>`, `\`, `@`, `!`, a comment or
`url()`.

A colour token that other colours follow also moves the colours derived
from it, unless the theme sets those too. For example, `content-bg` moves
the hover and divider colours.

## Part names

The CSS styles the app through the published part names in the contract,
such as `.aw-shell`, `.aw-sidebar`, `.aw-card`, `.aw-btn` and `.aw-composer`.
It can also style the regions `[data-region="rail" | "sidebar" | "content"]`.

These names are kept stable from one version to the next. Other class names
can change without notice.

## What CSS can't do

A theme never loads anything from elsewhere. Themes are shared with a whole
workspace, so the checks refuse:

- `url()`, `image()`, `image-set()` and any function not on the list;
- `@import`, `@font-face`, `@namespace` and any at-rule other than `@media`,
  `@supports`, `@container`, `@layer` and `@keyframes`;
- backslash escapes, `<`, control characters, unbalanced brackets or quotes,
  and unclosed comments;
- more than 32 KB of CSS, or a document over 64 KB.

Fonts are named in the `font-*` tokens and must already be on the device.

## Sharing

- A theme belongs to whoever made it. Only they change it; anyone else
  duplicates it to make a variation.
- Someone who manages the workspace's settings can offer a theme to everyone
  in the workspace (`workspace_theme_set`). They can also withdraw an offered
  theme or delete it.
- Export saves a theme as a `.theme.json` file. Import makes a new theme of
  your own from one.

## Agent tools

`theme_list`, `theme_get`, `theme_create`, `theme_update` (with
`expectedVersion`), `theme_delete` and `workspace_theme_set`. They are the
same operations the Appearance page runs.

## Keeping the app themeable

`apps/web/src/theme/contract.test.ts` holds these rules:

- Every token in the contract is defined in `tokens.css`.
- Every part name is styled somewhere in the app's CSS.
- No CSS outside `tokens.css` writes a colour out, unless its line says why
  with `/* design-exception: … */`.
- Every block in the shared pieces (`src/ui`, `src/pieces`) is a published
  part, or is listed as unpublished with a reason.

A new piece gets its part name in `contract.json`. Renaming a part is a new
contract `format`.
