# UI hill climb, round 0: tour and scorecard (AWD-84)

Mischa (2026-11-04): "It should feel like Slack, if Slack were a full
development and planning environment." The UI feels "kinda horrible" and
nobody has said exactly why. This round finds out. It changes nothing in the
app's look; round 1 (AWD-85) proposes whole-product directions.

## The tour

`tools/ui-tour/run.sh` takes 15 screens at 1440 px (the main score) and 390 px,
in light (Daylight) and dark (Midnight): 60 PNGs and a contact sheet in
`apps/web/e2e-output/tour/index.html`. From a clean checkout it:

- installs the web dependencies;
- builds the server under a memory cap;
- makes a throwaway database;
- seeds a busy workspace (`apps/web/e2e/tour/seed.ts`);
- takes the screenshots.

The tour has its own Playwright config (`playwright.tour.config.ts`) and is not
part of CI. `--only channel,page` takes some screens only.

The seed builds "Atlas Robotics", where Robin Achterberg is signed in:

- 8 people with long names. Each joined by invitation and posts as themselves.
- 4 agents with their own tokens.
- A project with a long name, plus three others.
- 204 tasks across every status, with labels, milestones, due dates, estimates and assignees.
- A task with a long description, acceptance criteria and comments.
- Busy channels with code, mentions, task links, reactions, and a thread with an agent in it.
- A DM and a group DM.
- Five pages, including a plan with a table, and a decision.
- A whiteboard of the ingest pipeline.
- Five agent runs: working, waiting for approval, done and failed.

Runs can't be made through the API without a repository prepared from GitHub,
so they are written straight into the throwaway database. So are message
times, which are spread over the last few hours.

## Scores

Each screen is scored out of 5 on seven things:

- **H**: hierarchy (where the eye goes first, and whether that's the right place).
- **D**: density (how much useful content fits; neither too cramped nor too airy).
- **S**: spacing and alignment.
- **T**: type.
- **C**: colour.
- **F**: how it feels.
- **Co**: coherence with the other screens.

1440 px light is the score; dark and 390 px notes follow each screen.

| # | Screen | H | D | S | T | C | F | Co | Avg |
|---|---|---|---|---|---|---|---|---|---|
| 1 | Home | 2 | 3 | 3 | 3 | 3 | 2 | 3 | 2.7 |
| 2 | Channel | 3 | 3 | 4 | 3 | 3 | 3 | 4 | 3.3 |
| 3 | DM | 4 | 3 | 4 | 3 | 3 | 3 | 4 | 3.4 |
| 4 | Thread | 3 | 3 | 3 | 3 | 3 | 3 | 3 | 3.0 |
| 5 | Task list | 3 | 4 | 3 | 3 | 2 | 3 | 3 | 3.0 |
| 6 | Board | 3 | 3 | 3 | 3 | 3 | 3 | 3 | 3.0 |
| 7 | Task | 4 | 3 | 3 | 4 | 4 | 3 | 3 | 3.4 |
| 8 | Run | 3 | 2 | 3 | 3 | 3 | 3 | 3 | 2.9 |
| 9 | Inbox | 2 | 2 | 3 | 3 | 3 | 2 | 3 | 2.6 |
| 10 | Search | 3 | 3 | 3 | 3 | 3 | 2 | 2 | 2.7 |
| 11 | Settings (members) | 3 | 3 | 3 | 3 | 3 | 3 | 3 | 3.0 |
| 12 | Page | 2 | 3 | 2 | 3 | 3 | 2 | 2 | 2.4 |
| 13 | Whiteboard | 3 | 3 | 3 | 2 | 3 | 3 | 1 | 2.6 |
| 14 | Command palette | 4 | 4 | 4 | 3 | 3 | 4 | 4 | 3.7 |
| 15 | Onboarding | 3 | 2 | 3 | 3 | 3 | 2 | 2 | 2.6 |
| | **Whole app** | 3.0 | 2.9 | 3.1 | 3.0 | 3.0 | 2.7 | 2.9 | **2.95** |

No screen is broken. Most are a solid 3: competent, tidy and unloved. The
worst are the ones that should feel calmest (a page, the inbox, onboarding)
and the ones that leave the app's frame (search, whiteboard).

### 1. Home (2.7)
- **Hierarchy:** the setup card takes the top of the page, above anything that needs you.
- **Equal weight:** two columns of bordered boxes look the same, so "Needs you", "Capacity" and "Your tasks" all weigh the same.
- **Repetition:** the full project name ("Fleet Telemetry & Release Engineering (Atlas platform)") sits under every row, so the meta line is longer than the title.
- **Low-value content:** "Capacity: Tour account, No usage yet" takes a whole card for one fact.
- **Placement:** the primary "New project" button is the loudest thing on screen. It isn't what Home is for.
- **Phone:** the same stack. "Working now" sits below the fold, behind five assignment rows.

### 2. Channel (3.3)
- **What works:** the timeline itself is good. Names lead, times are quiet, the agent tag is clear and reactions are compact.
- **Sidebar order:** the sidebar runs Overview…Runs, then Views, then Agents (five rows with "Idle" under each), then Channels with run threads nested under #agents-at-work. #releases, the channel you're in, is the last row at 1440×900.
- **Overlap:** the "24 new messages" pill sits on top of the first message.
- **Colour:** avatars use bright random fills, so a page of messages is a rainbow down the left edge.
- **Phone:** good. It is the most "same app" screen at 390.

### 3. DM (3.4)
- **What works:** calm and readable.
- **Intro:** the intro block (a 64 px avatar, a headline and a sentence) takes 200 px on every DM.
- **Sidebar:** the DM list mixes bold unread names with a group whose icon is a different size.
- **New line:** the red line runs edge to edge with a "New" tag. It's loud for what it says.

### 4. Thread (3.0)
- **Root message:** the root sits above a divider with no visual tie to the replies.
- **Header:** the Done chip, run icon and panel toggle are unlabelled icons on the right.
- **Composer:** a floating "Message / Run an agent" switch sits above it, plus a "Waiting for Release Steward to reply · Stop" line. That's three strips of chrome before you type.

### 5. Task list (3.0)
- **What works:** density is good, about 18 rows on screen.
- **Chrome:** about 190 px of title row, tabs and toolbar come before the first row.
- **Colour:** label dots and milestone chips compete with the title. Eight label colours carry no meaning.
- **Priority:** the "···" for no priority reads as a menu.
- **Avatars:** 20 px initials in random colours.
- **Missing structure:** status shows only as an icon, and there are no status groups.
- **Phone:** rows keep the title and key and drop labels, due date and priority.

### 6. Board (3.0)
- **Card height:** long titles make tall cards, so about 4 fit per column.
- **Columns:** Review is cut off at the right edge, and Done isn't visible.
- **Surfaces:** grey columns behind white cards add a third surface level.
- **Phone:** one column at a time works.

### 7. Task (3.4)
- **What works:** the best document in the app. The 28 px title, readable description and a properties panel on the right.
- **Weak actions:** "Add subtask · dependency · link · file · whiteboard" is a line of grey words that reads as a footnote.
- **Contradiction:** the Launch box says "No agent is set up for this project yet" while the sidebar lists five agents.
- **Criteria:** checked criteria are struck through and grey, so done reads as deleted.

### 8. Run (2.9)
- **Emptiness:** most of the page is empty sections: "Files changed: changes show up here…" and "Tests: 0 passed, 0 failed, 0 skipped".
- **Contradiction:** the test summary sits next to a step that reports a passing test.
- **Wasted table:** the detail table spends a 1,000 px row on "Model: Harness default".
- **Lost place:** it opens under the Home sidebar, not the project's, so you lose where you are.

### 9. Inbox (2.6)
- **Density:** every notification is its own card with a grey header strip, so 6 fit on screen.
- **Labels:** the strip's label is "a DM" (lowercase) or a truncated task title, with the full project name repeated beside it.
- **Empty pane:** the right half is an empty "Pick a notification" box.
- **Phone:** the strip label truncates to "#…" or "F…" while the long project name keeps its room.

### 10. Search (2.7)
- **No frame:** there's no sidebar at all here, so the frame jumps when you arrive.
- **Narrow column:** results are a narrow centred column in a lot of white.
- **Redundant button:** the "Search" button repeats what Enter does.
- **What works:** the results themselves are good, with kind chips and highlighted words.

### 11. Settings: members (3.0)
- **Long nav:** about 25 items in three groups, so the settings nav feels like a second app.
- **Order:** the invite form sits above the members, so the list starts at the fold.
- **Alignment:** the column is left-aligned in the space, leaving a strip of empty white on the right.

### 12. Page (2.4)
- **Duplicate title:** the title appears twice, as the page header and again as the H1 inside the body.
- **Boxed:** the body sits in a bordered card, so it's a form, not a page.
- **Header noise:** the header has a primary "Edit" button, an underlined "All pages" link and a version line.
- **Cramped table:** the table is narrow and dense, unlike the body text.
- **Alignment:** the column is centred but its icon tile hangs left of the text edge.
- **Overall:** this is where "Notion's calm writing surface" is furthest away.

### 13. Whiteboard (2.6)
- **Another product:** it looks like Excalidraw, with its hand-drawn font, toolbar with number hints, hint text, Library button and zoom pill. Coherence 1.
- **Wrapping:** shape text wraps mid-word ("telemetry_d / aily").
- **What works:** the app's own top bar (title, project chip, saved state, Link) is fine.

### 14. Command palette (3.7)
- **What works:** the best screen. Fast, dense, grouped, with keyboard hints.
- **Repetition:** rows repeat "FLT-185 · Fleet Telemetry & Release Engineer…" after every title.

### 15. Onboarding (2.6)
- **Disagreement:** it says "6 of 9 done" while Home said "1 of 9 done" until this page was opened.
- **Step markers:** the step list mixes checkmarks and numbers (1, 3, 6), so it doesn't read as one sequence.
- **No frame:** there's no sidebar, and the content is offset left of centre.
- **Empty space:** the form is three lines in an empty page.

### Dark (Midnight)
- **Consistent:** dark is consistent across screens.
- **Flat:** the sidebar, content and cards sit within a few lightness steps, so borders do all the separating. Hierarchy is weaker than in light.
- **Accent:** indigo buttons and tiles look heavier on dark.

### Phone (390 px)
- **The same app:** it reads as the same app, and that's the strongest consistency in the product.
- **Weak spots:**
  - The top bar truncates the long project name.
  - Page and task titles wrap to three or four lines.
  - Inbox strips truncate the wrong label.

## Top 10 problems

1. **No single frame.** The left side changes with every area: the project sidebar, the Home sidebar, the settings nav, or nothing (Search, Onboarding, Whiteboard). The content's left edge moves between 220 and 532 px. It feels like several apps.
2. **The project sidebar is overloaded and out of order.** Agent statuses and run threads push channels below the fold, and the channel you're in is the last row at 1440×900.
3. **Boxes everywhere.**
   - Cards inside sections.
   - A card for every inbox item.
   - A card around a page's body.
   - Grey board columns behind white cards.

   Everything looks equally important, and density suffers.
4. **Decorative page-title tiles.** Every page header has a lavender icon tile. With the indigo buttons, the accent is the loudest thing on screens where content should lead.
5. **Colour carries no meaning.**
   - Random avatar and project fills.
   - Eight rainbow label dots.
   - Three red "9+" badges on the rail at once.
   - Accent on checkboxes, tiles and buttons alike.

   Nothing tells the eye where to go.
6. **Flat type, long meta lines.** Almost everything is 14–15 px at 500–600 weight, with little contrast between levels. The full project name repeats under every Home, inbox, search and palette row.
7. **Pages aren't a calm writing surface.** The title appears twice, the body sits in a bordered card, and there's an Edit button, a version line and a cramped table.
8. **Heavy chrome on tasks.**
   - About 190 px of controls come before the first task.
   - Board cards grow tall and Done is off-screen.
   - Phone rows drop everything but the title.
9. **Empty and contradictory states read as broken.**
   - The run page says "0 passed" next to a passing test.
   - "No agent is set up" shows beside five agents.
   - Home says "1 of 9 done" while setup says "6 of 9".
10. **The whiteboard is another product.** Excalidraw's hand-drawn font and toolbar break coherence completely.

What already works and should survive any direction:

- the message timeline;
- the task document;
- the command palette;
- the 390 px drawer pattern;
- one font family;
- sentence-case copy.

## Found on the way (not fixed here)

Visual polish is round 1 and later, so these go on the board as tasks:

- Home's setup card counts the wrong project's steps until `/welcome` has been opened once ("1 of 9" against "6 of 9").
- On phones the inbox strip truncates its label ("#…", "F…") and keeps the project name whole.
- `POST /api/v1/projects/{id}/execution-profiles` answers 400 "The request is invalid" to every body, including `{}`, when the demo executor is on. The tour seeds its profile in SQL instead.

Fixed here: when `python3` is missing, the e2e setup now says so instead of
"bootstrap-admin failed (null): undefined".
