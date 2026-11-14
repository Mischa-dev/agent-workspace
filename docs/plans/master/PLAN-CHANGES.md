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
- **2026-11-14.** Architecture review AWD-150 weighed by the lead: rulings R52 to R59.
  No slice moved or removed. Request ids state intent (R52, with AWD-148); one public task
  revision (R53); one kind-name grammar (R54); pause per scope behind an opaque cursor
  (R55); remote and held operations without a held transaction (R56); observation gates run
  on the gate rehearsal (R57); coordination scenarios and prerequisite checks at gates
  (R58); several agents through one connector and an assistant that is both member and
  delegate, from existing contracts (R59).
