# Design System

## Theme

A restrained dark operational interface intended for a technical operator in a
mixed-light desktop environment. The visual system is deliberately provisional
during M0 because product naming and public identity are unresolved.

## Color

Use the existing cool blue-gray palette through CSS custom properties. Deep
neutral surfaces establish hierarchy; blue marks active work, orange marks a
human blocker, red marks failure, white marks newly completed work, and gray
marks settled history. Every state includes text and an icon in addition to
color. The accent is reserved for focus, selection, and primary actions.

## Typography

Use the existing system sans-serif stack for all interface text. Keep the scale
compact and fixed: 12–14px metadata and labels, 14–16px body and controls, and
16–20px page headings. Use the monospace stack only for commits, branches,
paths, and event payloads.

## Layout

The desktop shell has four regions: narrow project rail, project sidebar,
flexible main workspace, and collapsible context drawer. At medium widths the
context drawer disappears; at small widths the project sidebar also collapses.
The main Thread surface owns scrolling while the composer stays available.

## Components

- Project controls and Channels use one compact selection vocabulary.
- Timeline entries are grouped by meaning, not wrapped in identical cards.
- Provider events use quiet bordered rows with a visible event type and summary.
- Pending approvals use a contained alert surface with explicit allow and deny
  actions.
- Run status uses icon, label, and semantic color.
- Buttons, text areas, and focus rings share the same radii and state behavior.

## Motion

Use 150–200ms state transitions for hover, focus, and newly arriving timeline
content. Avoid page choreography. Disable nonessential transitions under
`prefers-reduced-motion`.
