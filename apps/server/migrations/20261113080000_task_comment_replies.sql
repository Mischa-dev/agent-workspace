-- AWD-95: reply to a task comment.
--
-- A reply is an ordinary comment on the same task that points at the one it
-- answers, the way a message reply (20261105040000) points at a message. The
-- quote is read live from the original, never copied, so an edit or a
-- deletion of the original shows in the quote too.
--
-- The foreign key on (project_id, work_item_id, reply_to_comment_id) means a
-- reply can only point at a comment on its own task. Comments are never
-- deleted from the table (a deletion blanks them), so the key needs no
-- ON DELETE.
ALTER TABLE work_item_comments
    ADD COLUMN reply_to_comment_id UUID,
    ADD CONSTRAINT work_item_comments_reply_to_same_task_fk
        FOREIGN KEY (project_id, work_item_id, reply_to_comment_id)
        REFERENCES work_item_comments (project_id, work_item_id, id),
    ADD CONSTRAINT work_item_comments_reply_to_other_comment_check
        CHECK (reply_to_comment_id IS DISTINCT FROM id);

-- What a comment replies to is fixed once it's posted; an edit changes only
-- the text.
CREATE FUNCTION guard_work_item_comment_reply_to()
RETURNS TRIGGER LANGUAGE plpgsql AS $$
BEGIN
    IF NEW.reply_to_comment_id IS DISTINCT FROM OLD.reply_to_comment_id THEN
        RAISE EXCEPTION 'What a task comment replies to cannot change'
            USING ERRCODE = '23514';
    END IF;
    RETURN NEW;
END;
$$;

CREATE TRIGGER work_item_comments_guard_reply_to
BEFORE UPDATE OF reply_to_comment_id ON work_item_comments
FOR EACH ROW EXECUTE FUNCTION guard_work_item_comment_reply_to();

CREATE INDEX work_item_comments_reply_to ON work_item_comments (reply_to_comment_id)
    WHERE reply_to_comment_id IS NOT NULL;
