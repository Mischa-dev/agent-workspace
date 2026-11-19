---
name: write-pages-and-memory
description: Keep what is true now in pages and decisions, so nobody has to reread a conversation.
---

# Write pages and decisions

- Pages hold what is true now and keep every version: read with `page_get`,
  make one with `page_create` and change it with `page_update`, or block by
  block with `page_blocks_get` and `page_update_blocks`.
- When a discussion lands, record it with `decision_record`.
- Read a project's guide page (from `project_get`) before you change
  things there.
