# The master plan

**Reference only since 2026-11-16.** The plan is the one page "Build plan" in the app's
AWD project, with a task per lane; see `AGENTS.md`. This folder is the detailed thinking
behind it: look things up here for background, and overrule it where the code says
otherwise. Nobody has to read it first, keep it current or mirror it. `END-STATE.md` is
still what the product is becoming. What is here:

1. `END-STATE.md`: the finished product, its principles and its words.
2. `LEAD-RULINGS.md`: decisions that settle disagreements. A ruling beats plan text.
3. `BUILD-ORDER.md`: the 31 builds in six stages, how to run a build (section 3), and
   what each build contains (section 4).
4. `plans/<id>-*.md`: the fourteen detailed plans, one section per slice.

`slices.json` lists the slices and builds the plans were first cut into; the lane tasks'
feature lists came from it, and the ids in them are lookup keys into these plans. It and
`tools/plan.py` are frozen: CI no longer runs the checker.

Some documents mention research notes and cross-plan reviews (`R1` to `R3`, `review/`,
`PLAN-BRIEF.md`, `HANDBACKS.md`). They were working papers for writing these plans and
stay outside the repository; nothing here depends on them.
