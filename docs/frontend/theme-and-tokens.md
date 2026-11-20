# Theme and token contract

## Preference model

`ThemeProvider` owns presentation preferences only:

- theme: a built-in theme (`src/theme/slots.ts` `THEME_PRESETS`) or `system`
  (Quiet in light mode, Quiet dark in dark mode)
- customTheme: the id and workspace of a custom theme, shown instead of
  `theme` while it's set; its document is cached under
  `agent-workspace:theme-document:v1` and read again with `theme_get`
- mode: `system`, `light` or `dark`, which half of a custom theme shows
- overrides: colors someone changed on a built-in theme before custom themes
  existed; `CustomThemeSync` turns them into a custom theme, "My colors", once
- density: `comfortable` or `compact`
- text size: `small`, `default` or `large`

The local key is `agent-workspace:appearance:v2`. A `v1` value is migrated on
read (dark becomes Midnight, anything else Indigo). Parsing fails closed:
unknown presets and anything malformed are dropped. Credentials, sessions,
actors, projects, capabilities, drafts and server responses are never stored
here.

## Resolution

The provider sets `data-theme` (the built-in theme underneath),
`data-density` and `data-text-size` on `<html>`. A custom theme's tokens for
the mode that shows are written as inline custom properties; a base color
token it changes also moves the tokens derived from it (`DERIVED_SLOTS` in
`src/theme/slots.ts`, for example `sidebar-hover` from `sidebar-bg`) unless
the theme sets those too. Its CSS goes in one `<style>` last in `<head>`, so
it wins ties. `public/theme-boot.js` applies the cached theme before the app
loads, so the first paint is already in the person's theme; keep it in step
with `preferences.ts` and `customTheme.ts`.

What a theme document may hold, and the checks the server and the web run
on it, are in `docs/theming.md`.

## Token layers

1. Built-in themes: the `[data-theme]` blocks in `src/styles/tokens.css`
   (Paper and Frame in `public/themes/paper-frame.css`, the classic palettes
   in `public/themes/classic.css`), contrast-checked for every pair.
2. A custom theme's tokens, inline on `<html>`, then its CSS.
3. Regions: `--r-*` variables set by `data-region` on the rail, sidebar and
   content columns.
4. Components read slots and region variables only; no literal colors.
5. Scales shared by every theme: type (`--t-*`, `--weight-*`), spacing
   (`--space-*`), corners (`--radius-*`), focus (`--focus-*`,
   `--shadow-focus`), motion (`--duration-*`, `--ease-*`) and layers
   (`--layer-*`). `apps/web/DESIGN.md` says which to use where, and
   `src/styles/designSystem.test.ts` keeps stylesheets on them.

Every page now uses these tokens directly. The pre-redesign aliases
(`--color-canvas`, `--text`, `--panel` and so on) were removed in pass two, so
don't reintroduce them.

## Contrast

`CONTRAST_PAIRS` lists the pairs that carry meaning: text at 4.5:1, icons,
borders, focus rings and dots at 3:1. The theme editor checks them on every
change, flags the pair and offers Fix, which walks the text color toward black
or white until the pair passes.
