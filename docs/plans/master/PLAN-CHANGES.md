# Plan changes

Every change to a build after it was first written: the date, what changed in `slices.json`,
and why (R50). Newest last.

- **2026-11-14.** The wave order (16 waves, one pull request per slice) became 31 builds in
  six stages (R44), at the owner's request. No slice removed. What moved and why is in
  `BUILD-ORDER.md` section 10; the old order is `archive/BUILD-ORDER.waves.md`.
- **2026-11-14.** No deploy until the plan is done (R51, the owner's choice). Build 5X no
  longer waits for a release; contract steps may follow their replacement at once. A
  deploy rehearsal on a copy of the live database is added at every stage gate. Aliases
  for outside clients count from the one deploy.
- **2026-11-14.** T1.12 (task tools take `fields` and `collection`) and T1.25 (acts like a
  task) were done early by #268 (AWD-110 steps 5 and 6), before build 1T. Both stay in build
  4T in `slices.json`, marked done early; T1.25's leftovers (forms for task-like collections,
  switching off once it has records, per-collection workflows, the switch in workspace
  templates) are named on it for build 4T to finish.
