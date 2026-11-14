# The master plan

Where Agent Workspace is going and the order it is built in. Read in this order:

1. `END-STATE.md`: the finished product, its principles and its words.
2. `LEAD-RULINGS.md`: decisions that settle disagreements. A ruling beats plan text.
3. `BUILD-ORDER.md`: the 31 builds in six stages, how to run a build (section 3), and
   what each build contains (section 4).
4. `plans/<id>-*.md`: the fourteen detailed plans, one section per slice.

`slices.json` is the one list of slices, builds, needs and milestones. `tools/plan.py`
checks it against the plans (`python3 docs/plans/master/tools/plan.py`) and, with
`--write`, regenerates `SLICES.md` and the generated parts of `BUILD-ORDER.md`. Never edit
those parts by hand. Every change to a build after it was first written gets a dated line
in `PLAN-CHANGES.md`.

Some documents mention research notes and cross-plan reviews (`R1` to `R3`, `review/`,
`PLAN-BRIEF.md`, `HANDBACKS.md`). They were working papers for writing these plans and
stay outside the repository; nothing here depends on them.
