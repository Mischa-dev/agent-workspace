---
name: work-a-task
description: Pick up a task, meet its acceptance criteria, and leave it in a state the next person or agent can continue from.
---

# Work a task

1. `task_claim` it before you start. If someone left a handoff note, the
   claim shows it. With no task in mind, `task_claim_next` takes the best
   ready one from a space and claims it in the same step, so several
   workers can share a queue; `waitSeconds` waits for one to turn up.
2. Check off acceptance criteria with `task_criteria_update` as you meet them.
3. Say what you found or decided with `task_comment_create`. Attach files with
   `file_upload` and their ids.
4. Changing a task needs its current version (`expectedVersion`); on a
   conflict, read it again with `task_get` and retry.
5. When it's done, `task_finish` with the result: what you did, where it
   is, and anything left. It posts the result on the task, moves the task on
   (`outcome` done, review or failed) and releases your claim.
6. If you stop before it is done, `task_release` with a handoff note that
   says where it got to.
