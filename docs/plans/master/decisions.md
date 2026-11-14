# Owner decisions

One row per owner question from the workstream plans. A question nobody has
answered yet proceeds on the planner's recommendation and says so ("assumed:
…"); when the owner answers, the row records the answer and the date, and the
slices it unblocks follow it.

| Id | Question | Recommendation | Answer | Date | Slices |
| --- | --- | --- | --- | --- | --- |
| P0 Q1 | Is a role's `modules_off` a permission (people with it must not reach that module's pages over the API) or only a view? | A permission, as MCP already treats it; the HTTP gate applies role narrowing too | assumed: a permission | 2026-11-14 | P0.7, K1.11 |
| P0 Q2 | Is any database still running from before migration 0010 (an M0-M2 install)? | If not, retire the five upgrade replays and keep everything that runs live code | assumed: none is; retire the five replays | 2026-11-14 | P0.25 |
| P0 Q3 | May the research files and the context file go into `docs/plans/master/research/` as they are? | Only the end state and the plans go in now; research after the lead strips owner-specific detail | assumed: only the end state and the plans now | 2026-11-14 | P0.30 |
| P0 Q4 | Should `tools/ci/run.sh` (routing to personal machines) stay in this repository? | Keep it, with hosts read from an untracked file, so `checks.sh` stays the portable entry point | assumed: keep it, hosts from an untracked file | 2026-11-14 | P0.33 |
| P0 Q5 | Until T1 merges the task and collection view engines, may new task view, board and filter features be built only on the collections side? | Yes | assumed: yes, collections side only | 2026-11-14 | P0.26 rule 4, T1.1 |
