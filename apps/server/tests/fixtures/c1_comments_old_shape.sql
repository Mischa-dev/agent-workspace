-- Old-shape comments: a task with a comment, a reply, an edited one, a
-- removed one, a file and a notification; a page with an anchored,
-- resolved thread (with a reply and a deleted comment) and a whole-page one.
INSERT INTO organizations (id, name, slug)
VALUES ('00000000-0000-7000-8000-000000000001', 'Old', 'old');
INSERT INTO actors (id, kind) VALUES
    ('00000000-0000-7000-8000-000000000002', 'human'),
    ('00000000-0000-7000-8000-000000000003', 'human');
INSERT INTO organization_memberships (organization_id, actor_id, role, enabled) VALUES
    ('00000000-0000-7000-8000-000000000001', '00000000-0000-7000-8000-000000000002', 'owner', true),
    ('00000000-0000-7000-8000-000000000001', '00000000-0000-7000-8000-000000000003', 'member', true);
INSERT INTO projects (id, organization_id, project_key, name)
VALUES ('00000000-0000-7000-8000-000000000010', '00000000-0000-7000-8000-000000000001', 'OLD', 'Old');
INSERT INTO project_memberships (id, project_id, actor_id, role, display_name, handle) VALUES
    (gen_random_uuid(), '00000000-0000-7000-8000-000000000010', '00000000-0000-7000-8000-000000000002', 'owner', 'Owner', 'owner'),
    (gen_random_uuid(), '00000000-0000-7000-8000-000000000010', '00000000-0000-7000-8000-000000000003', 'member', 'Bea', 'bea');
INSERT INTO project_counters (project_id) VALUES ('00000000-0000-7000-8000-000000000010');
INSERT INTO work_item_types (id, project_id, name, position, is_default)
VALUES ('00000000-0000-7000-8000-000000000011', '00000000-0000-7000-8000-000000000010', 'Task', 1, true);
INSERT INTO work_item_statuses (id, project_id, name, semantic_category, position, is_default)
VALUES ('00000000-0000-7000-8000-000000000012', '00000000-0000-7000-8000-000000000010', 'Todo', 'unstarted', 1, true);

-- The task, its record first.
INSERT INTO collection_records (id, project_id, collection_id, number, title,
                                created_by_actor_id, updated_by_actor_id)
VALUES ('00000000-0000-7000-8000-000000000020', '00000000-0000-7000-8000-000000000010',
        ensure_tasks_collection('00000000-0000-7000-8000-000000000010', '00000000-0000-7000-8000-000000000002'),
        1, 'Fix the bridge', '00000000-0000-7000-8000-000000000002', '00000000-0000-7000-8000-000000000002');
INSERT INTO work_items (id, project_id, number, type_id, status_id, created_by_actor_id, board_rank,
                        next_timeline_sequence)
VALUES ('00000000-0000-7000-8000-000000000020', '00000000-0000-7000-8000-000000000010', 1,
        '00000000-0000-7000-8000-000000000011', '00000000-0000-7000-8000-000000000012',
        '00000000-0000-7000-8000-000000000002', 1024, 5);
UPDATE project_counters SET next_work_item_number = 2
WHERE project_id = '00000000-0000-7000-8000-000000000010';

-- Its comments: the first, Bea's reply (edited once), and a removed one.
INSERT INTO work_item_comments (id, project_id, work_item_id, author_actor_id, content, plain_text,
                                created_at, revision, edited_at, removed_at, removed_by_actor_id,
                                reply_to_comment_id)
VALUES
    ('00000000-0000-7000-8000-000000000031', '00000000-0000-7000-8000-000000000010',
     '00000000-0000-7000-8000-000000000020', '00000000-0000-7000-8000-000000000002',
     '{"version": 1, "nodes": [{"type": "text", "text": "Steel or wood?"}]}', 'Steel or wood?',
     '2026-11-01 10:00+00', 1, NULL, NULL, NULL, NULL),
    ('00000000-0000-7000-8000-000000000032', '00000000-0000-7000-8000-000000000010',
     '00000000-0000-7000-8000-000000000020', '00000000-0000-7000-8000-000000000003',
     '{"version": 1, "nodes": [{"type": "text", "text": "Steel, surely"}]}', 'Steel, surely',
     '2026-11-01 11:00+00', 2, '2026-11-01 11:05+00', NULL, NULL,
     '00000000-0000-7000-8000-000000000031'),
    ('00000000-0000-7000-8000-000000000033', '00000000-0000-7000-8000-000000000010',
     '00000000-0000-7000-8000-000000000020', '00000000-0000-7000-8000-000000000003',
     '{"nodes": [], "version": 1}', '',
     '2026-11-01 12:00+00', 1, NULL, '2026-11-01 12:30+00', '00000000-0000-7000-8000-000000000003', NULL);
ALTER TABLE work_item_comment_revisions DISABLE TRIGGER USER;
INSERT INTO work_item_comment_revisions (workspace_id, project_id, work_item_id, comment_id, revision,
                                         content, plain_text, written_by_actor_id, written_at)
VALUES
    ('00000000-0000-7000-8000-000000000001', '00000000-0000-7000-8000-000000000010',
     '00000000-0000-7000-8000-000000000020', '00000000-0000-7000-8000-000000000032', 1,
     '{"version": 1, "nodes": [{"type": "text", "text": "Steel"}]}', 'Steel',
     '00000000-0000-7000-8000-000000000003', '2026-11-01 11:00+00'),
    ('00000000-0000-7000-8000-000000000001', '00000000-0000-7000-8000-000000000010',
     '00000000-0000-7000-8000-000000000020', '00000000-0000-7000-8000-000000000032', 2,
     '{"version": 1, "nodes": [{"type": "text", "text": "Steel, surely"}]}', 'Steel, surely',
     '00000000-0000-7000-8000-000000000003', '2026-11-01 11:05+00');
ALTER TABLE work_item_comment_revisions ENABLE TRIGGER USER;
INSERT INTO work_item_timeline_entries (project_id, work_item_id, sequence, kind, comment_id, created_at)
VALUES
    ('00000000-0000-7000-8000-000000000010', '00000000-0000-7000-8000-000000000020', 2, 'comment',
     '00000000-0000-7000-8000-000000000031', '2026-11-01 10:00+00'),
    ('00000000-0000-7000-8000-000000000010', '00000000-0000-7000-8000-000000000020', 3, 'comment',
     '00000000-0000-7000-8000-000000000032', '2026-11-01 11:00+00'),
    ('00000000-0000-7000-8000-000000000010', '00000000-0000-7000-8000-000000000020', 4, 'comment',
     '00000000-0000-7000-8000-000000000033', '2026-11-01 12:00+00');
INSERT INTO files (id, organization_id, project_id, work_item_id, comment_id, uploaded_by_actor_id,
                   name, content_type, byte_size, sha256, storage_key, attached_at)
VALUES ('00000000-0000-7000-8000-000000000040', '00000000-0000-7000-8000-000000000001',
        '00000000-0000-7000-8000-000000000010', '00000000-0000-7000-8000-000000000020',
        '00000000-0000-7000-8000-000000000031', '00000000-0000-7000-8000-000000000002',
        'bridge.png', 'image/png', 10, repeat('0', 64), '0000/0040', now());
INSERT INTO notifications (id, project_id, recipient_actor_id, kind, subject_kind, subject_id, actor_id,
                           work_item_id, comment_id, group_key)
VALUES ('00000000-0000-7000-8000-000000000050', '00000000-0000-7000-8000-000000000010',
        '00000000-0000-7000-8000-000000000002', 'task_changed', 'work_item',
        '00000000-0000-7000-8000-000000000020', '00000000-0000-7000-8000-000000000003',
        '00000000-0000-7000-8000-000000000020', '00000000-0000-7000-8000-000000000032',
        'watch:00000000-0000-7000-8000-000000000020');
INSERT INTO notification_occurrences (recipient_actor_id, dedupe_key, notification_id)
VALUES ('00000000-0000-7000-8000-000000000002', 'watch:00000000-0000-7000-8000-000000000032',
        '00000000-0000-7000-8000-000000000050');

-- The page: an anchored thread, resolved, with a reply and a deleted
-- comment; and a comment on the whole page.
INSERT INTO project_pages (id, project_id, title, body, created_by_actor_id, updated_by_actor_id)
VALUES ('00000000-0000-7000-8000-000000000060', '00000000-0000-7000-8000-000000000010',
        'Plan', 'Ship the first slice.', '00000000-0000-7000-8000-000000000002',
        '00000000-0000-7000-8000-000000000002');
INSERT INTO project_page_comments (id, project_id, page_id, thread_id, author_actor_id, body, block_id,
                                   quote, resolved_at, resolved_by_actor_id, created_at, edited_at,
                                   deleted_at)
VALUES
    ('00000000-0000-7000-8000-000000000061', '00000000-0000-7000-8000-000000000010',
     '00000000-0000-7000-8000-000000000060', '00000000-0000-7000-8000-000000000061',
     '00000000-0000-7000-8000-000000000003', 'Which slice is first?', 'block1', 'first slice',
     '2026-11-02 09:00+00', '00000000-0000-7000-8000-000000000002', '2026-11-02 08:00+00', NULL, NULL),
    ('00000000-0000-7000-8000-000000000062', '00000000-0000-7000-8000-000000000010',
     '00000000-0000-7000-8000-000000000060', '00000000-0000-7000-8000-000000000061',
     '00000000-0000-7000-8000-000000000002', 'The billing one', NULL, NULL,
     NULL, NULL, '2026-11-02 08:30+00', '2026-11-02 08:31+00', NULL),
    ('00000000-0000-7000-8000-000000000063', '00000000-0000-7000-8000-000000000010',
     '00000000-0000-7000-8000-000000000060', '00000000-0000-7000-8000-000000000061',
     '00000000-0000-7000-8000-000000000003', '', NULL, NULL,
     NULL, NULL, '2026-11-02 08:40+00', NULL, '2026-11-02 08:45+00'),
    ('00000000-0000-7000-8000-000000000064', '00000000-0000-7000-8000-000000000010',
     '00000000-0000-7000-8000-000000000060', '00000000-0000-7000-8000-000000000064',
     '00000000-0000-7000-8000-000000000002', 'Looks good overall', NULL, NULL,
     NULL, NULL, '2026-11-02 10:00+00', NULL, NULL);
INSERT INTO notifications (id, project_id, recipient_actor_id, kind, subject_kind, subject_id, actor_id,
                           detail, group_key)
VALUES ('00000000-0000-7000-8000-000000000051', '00000000-0000-7000-8000-000000000010',
        '00000000-0000-7000-8000-000000000002', 'page_comment', 'page',
        '00000000-0000-7000-8000-000000000060', '00000000-0000-7000-8000-000000000003',
        '{"pageCommentId": "00000000-0000-7000-8000-000000000061", "threadId": "00000000-0000-7000-8000-000000000061"}',
        'page-comments:00000000-0000-7000-8000-000000000060');
