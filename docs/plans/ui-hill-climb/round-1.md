# UI hill climb, round 1: three directions (AWD-85, draft)

Three whole-product directions, each a set of token changes plus a few kit
rules in `apps/web/src/styles/directions.css`. They're switched by
`data-direction` on `<html>` (`src/theme/direction.ts`):

- `?direction=workroom`, `ledger` or `paper` picks one and remembers it.
- `?direction=current` goes back to today's look.

To shoot one: `tools/ui-tour/run.sh --direction ledger --only channel,task-list,page`.

Mischa picks one (or a mix). Then the winner moves into `tokens.css`, and the
switch and this branch go away. Don't merge this branch.

## A. Workroom: conversation first

**Borrows Slack's ease with conversation.**

- **One frame:** a confident ink frame (the rail and sidebar as one block) around a bright room.
- **Colour:** a warm teal accent, and amber mentions.
- **Timeline:** bold 15 px names.
- **Shape:** rounder corners.

**Why it's still one product:** tasks and pages sit in the same room inside the
same frame, so the frame is the identity and every screen inherits it.

## B. Ledger: fast and dense

**Borrows Linear's speed and density.**

- **Surface:** one neutral grey surface from rail to content, with hairlines instead of boxes.
- **Type and rows:** type a step smaller (13 px controls, 14 px body), 28 px rows, a slimmer rail and sidebar.
- **Colour:** muted identity colours. The accent is kept for the one action, status and mentions.
- **Dark:** the hero.

**Why it's still one product:** the density rule applies to chat and pages as
much as tasks, so a message, a task row and a paragraph share one rhythm.

## C. Paper: calm and document-first

**Borrows Notion's calm writing surface.**

- **Surface:** warm paper and ink, with no boxes around reading.
- **Type:** Source Serif 4 for page and document titles, and a 16 px reading measure in pages.
- **Colour:** black ink for the one primary action, and earthy identity colours.

**Why it's still one product:** the serif is used only for titles, everywhere,
and the same paper and ink run through chat and tasks. A channel feels like
the same calm place, not a different app.

## What all three share (and what they don't fix)

All three:

- keep Figtree for the interface;
- keep one kit and the same layout;
- drop or quiet the decorative page-title tiles;
- take the bordered card off pages.

Structural problems from round 0 are the same in every direction, and rounds 2
onwards take them on:

- the overloaded sidebar order;
- the disappearing frame on Search and setup;
- inbox cards;
- empty run sections.
