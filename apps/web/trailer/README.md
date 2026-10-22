# Product trailer

A 90-second animated trailer of Agent Workspace: planning a project as tasks, teammates and agents working in the
same channels, handing a task to an agent and getting a pull request back, and bringing any agent you like. It was
made in October 2026, then reworked so every change on screen moves smoothly and each agent shows its own logo.

The rendered video isn't kept in git, since a 30 MB file would stay in the history for good. `./make.sh` renders it.

## How it's made

The trailer is a React page that draws each frame from the app's own components, stylesheets and design tokens
(`apps/web/src/ui`, `apps/web/src/styles/tokens.css`), with demo data only. `window.__trailer.seek(t)` puts the page
at any moment, so frames are captured one by one with Playwright and encoded with ffmpeg. The soundtrack and the
interface sounds are synthesized in `soundtrack.py` (numpy and scipy, no samples or licensed music) and follow the
scene times in `src/Trailer.tsx` and each scene file.

Seeking finishes every CSS transition and animation at once, so nothing may rely on them: every change on screen is
drawn from the time. `src/parts/Swap.tsx` cross-fades anything that changes in place (a status, a chip, a line of
text) while easing its size, `Arrive` in `src/parts/Chat.tsx` opens a new row into a list (and with `out`, fades it
and folds it away again), and `WritingRow` lays an agent's reply out in full while it streams so the timeline never
jumps a line at a time. Give these `progress()` values on its default curve: `opening()` in `src/motion.ts` turns
them into a size that starts and ends gently, and other curves (or `1 - progress(...)` for a leaving row) make the
size snap. Where a row opens or closes inside a grid with a gap, the gap can't shrink with it (a grid row is never
shorter than nothing), so the neighbouring row is pulled into the gap by a negative margin instead: see the board's
slots in `src/parts/Tasks.tsx` and the whiteboards and launch card in `src/parts/TaskPage.tsx`.

Agent logos come from Lobe Icons (`@lobehub/icons-static-svg` 1.97.0, MIT, license in `assets/logos/LICENSE`),
composed into square tiles in `assets/logos/` by `assets/logos/tiles.py`. The marks belong to their makers. The trailer's Vite config swaps the
app's `src/ui/identity.tsx` for `src/identity.tsx`, so every avatar the app's own components draw shows the agent's
logo, looked up by its id in `src/logos.ts`. People keep the app's initials avatars. Whiteboard sketches use
Excalidraw's own rough.js and Excalifont, read from the installed package (`vite.config.ts`).

## Rebuilding it

With the web dependencies installed and the Node in `.node-version` on `PATH` (in a cloud session,
`tools/cloud/setup.sh` does both), plus ffmpeg and `pip install scipy`. Where Playwright's own Chromium isn't
installed, point `PLAYWRIGHT_CHROMIUM_EXECUTABLE` at one (`/opt/pw-browsers/chromium` in a cloud session).

```
cd apps/web/trailer
../node_modules/.bin/tsc -p tsconfig.json   # type-check it
./shots.sh check 12 40 61                   # stills of any moments, into stills/check/
node probe.mjs 'document.title' 12 40       # evaluate an expression at moments (on the last build), to check layout
./make.sh /tmp/trailer-out                  # the full render (about 8 minutes), soundtrack, master and web copy
python3 -I tools/motion.py /tmp/trailer-out/video.mp4 /tmp/trailer-out           # every frame against the last
python3 -I tools/popframes.py /tmp/trailer-out/video.mp4 /tmp/trailer-out 40.17  # the frames around one jump
```

`motion.py` lists frames that change much more than the ones around them, and stretches where nothing moves. The only
ones left now are one-pixel text snaps where a slow move starts or ends, and the first frame of a quick fade.

The trailer imports the app's components directly but isn't part of the app's build, lint or tests, so a change to
the app can break it without CI noticing: type-check it before rendering. The cut lives in `src/Trailer.tsx` (scene
list and times); each scene in `src/scenes/` keeps its own beats in a `T` table. If a scene's timing changes, move
the matching sound cues in `soundtrack.py` too.
