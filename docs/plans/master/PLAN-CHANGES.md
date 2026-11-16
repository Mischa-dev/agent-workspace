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
- **2026-11-14.** Architecture review AWD-150 weighed by the lead: rulings R52 to R59.
  No slice moved or removed. Request ids state intent (R52, with AWD-148); one public task
  revision (R53); one kind-name grammar (R54); pause per scope behind an opaque cursor
  (R55); remote and held operations without a held transaction (R56); observation gates run
  on the gate rehearsal (R57); coordination scenarios and prerequisite checks at gates
  (R58); several agents through one connector and an assistant that is both member and
  delegate, from existing contracts (R59).
- **2026-11-14.** The roadmap coverage check (is every item of the earlier roadmap and the
  Linear, Notion and Slack build-out plan in this plan?) found 12 gaps and some partial
  coverage; the lead folded them in. New slices: A1.24 plan windows over the host protocol
  (3G, before A1.17, which now needs it), A1.25 a default assistant in the built-in host
  (4G), W1.40 UI hill climb round 1 (2W), W1.41 later hill-climb rounds (4W), W1.42
  automation builder (4W), C1.38 pages inside pages (4D), C2.41 search by meaning (4C),
  K3.26 incoming webhooks as a trigger (3E), T1.42 milestone order and roll-up (4T), T1.43
  project status and update posts (4T). Acceptance added to A1.5, A1.17, K2.14, K4.7,
  W1.24, C1.29 and C1.35. T1 notes that #262, #265 and #268 already did tasks-into-records
  steps 2, 3, 5 and 6. Admin retention rules and moving AI suggestions onto requests are
  open questions in `decisions.md` (Lead Q1, Q2). No slice removed.
