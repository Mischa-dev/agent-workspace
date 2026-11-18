---
name: report-your-work
description: Report a piece of work as a run, so people see its steps, results and usage in one place.
---

# Report your work

1. `run_create` with a stable `clientRunId`, a title and its cause. A retry
   returns the same run.
2. Report steps and messages with `run_activity_report`; each activity has its own
   id, so a retried batch is stored once.
3. Say what it is doing now with `run_update` (`statusLine`).
4. Report what it produced with `run_result_report` (links, files, pull requests).
5. Finish with an activity of kind "run_finished" carrying the outcome.
