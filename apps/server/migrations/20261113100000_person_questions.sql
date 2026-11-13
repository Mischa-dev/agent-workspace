-- AWD-62: an agent asks a person something on an inbox card, and the answer
-- goes back to it.
--
-- A question is asked by one actor (usually an agent) of one person in a
-- project. It can offer up to four answers to pick from, and the person can
-- always type their own. It may be about a conversation or a task in the
-- same project, which the composite keys enforce; the person is only told
-- about it while they can see that context (the inbox's own checks).
--
-- The answer is kept on the row: the asker reads it with question_list, and
-- each of its connections gets a question.answered event.
CREATE TABLE person_questions (
    id UUID PRIMARY KEY,
    project_id UUID NOT NULL REFERENCES projects(id) ON DELETE CASCADE,
    asked_by_actor_id UUID NOT NULL REFERENCES actors(id),
    person_actor_id UUID NOT NULL REFERENCES actors(id),
    question TEXT NOT NULL CHECK (
        char_length(btrim(question)) BETWEEN 1 AND 2000
        AND question !~ '[\x01-\x08\x0b\x0c\x0e-\x1f\x7f]'
    ),
    -- [{"id": "1", "label": "Yes"}, ...]; empty when only a typed answer fits.
    options JSONB NOT NULL DEFAULT '[]'::jsonb CHECK (
        jsonb_typeof(options) = 'array'
        AND jsonb_array_length(options) <= 4
        AND octet_length(options::text) <= 2000
    ),
    conversation_id UUID,
    work_item_id UUID,
    status TEXT NOT NULL DEFAULT 'pending' CHECK (status IN ('pending', 'answered')),
    -- The option picked, if one was.
    answer_option_id TEXT CHECK (answer_option_id IS NULL OR char_length(answer_option_id) <= 8),
    -- The words typed, if any (with or without an option).
    answer_text TEXT CHECK (
        answer_text IS NULL OR char_length(btrim(answer_text)) BETWEEN 1 AND 4000
    ),
    answered_at TIMESTAMPTZ,
    created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    CONSTRAINT person_questions_asks_someone_else CHECK (asked_by_actor_id <> person_actor_id),
    CONSTRAINT person_questions_answer_shape CHECK (
        (status = 'pending' AND answered_at IS NULL
            AND answer_option_id IS NULL AND answer_text IS NULL)
        OR (status = 'answered' AND answered_at IS NOT NULL
            AND (answer_option_id IS NOT NULL OR answer_text IS NOT NULL))
    ),
    CONSTRAINT person_questions_conversation_same_project
        FOREIGN KEY (project_id, conversation_id)
        REFERENCES conversations (project_id, id) ON DELETE CASCADE,
    CONSTRAINT person_questions_work_item_same_project
        FOREIGN KEY (project_id, work_item_id)
        REFERENCES work_items (project_id, id) ON DELETE CASCADE
);

-- An answer is final.
CREATE FUNCTION guard_person_question_answer()
RETURNS TRIGGER LANGUAGE plpgsql AS $$
BEGIN
    IF OLD.status = 'answered' THEN
        RAISE EXCEPTION 'An answered question cannot change'
            USING ERRCODE = '23514';
    END IF;
    IF NEW.project_id IS DISTINCT FROM OLD.project_id
        OR NEW.asked_by_actor_id IS DISTINCT FROM OLD.asked_by_actor_id
        OR NEW.person_actor_id IS DISTINCT FROM OLD.person_actor_id
        OR NEW.question IS DISTINCT FROM OLD.question
        OR NEW.options IS DISTINCT FROM OLD.options
        OR NEW.conversation_id IS DISTINCT FROM OLD.conversation_id
        OR NEW.work_item_id IS DISTINCT FROM OLD.work_item_id THEN
        RAISE EXCEPTION 'Only the answer to a question can change'
            USING ERRCODE = '23514';
    END IF;
    RETURN NEW;
END;
$$;

CREATE TRIGGER person_questions_guard_answer
BEFORE UPDATE ON person_questions
FOR EACH ROW EXECUTE FUNCTION guard_person_question_answer();

-- The person's open questions, and what an asker has asked lately.
CREATE INDEX person_questions_for_person
    ON person_questions (person_actor_id, created_at DESC)
    WHERE status = 'pending';
CREATE INDEX person_questions_asked_by
    ON person_questions (asked_by_actor_id, created_at DESC);

-- A question.answered event is read only by the one who asked.
CREATE OR REPLACE FUNCTION external_event_visible_to(connection_uuid UUID, resource_type TEXT, resource_uuid UUID)
RETURNS boolean LANGUAGE sql STABLE AS $$
    SELECT CASE resource_type
        WHEN 'message' THEN EXISTS (SELECT 1 FROM messages m WHERE m.id = resource_uuid AND conversation_visible_to(m.conversation_id, c.actor_id))
        WHEN 'reply_request' THEN EXISTS (SELECT 1 FROM reply_requests r WHERE r.id = resource_uuid AND r.agent_actor_id = c.actor_id AND conversation_visible_to(r.conversation_id, c.actor_id))
        WHEN 'agent_approval' THEN EXISTS (SELECT 1 FROM agent_approvals a JOIN agent_runs r ON r.id = a.agent_run_id WHERE a.id = resource_uuid AND conversation_visible_to(r.conversation_id, c.actor_id))
        WHEN 'conversation' THEN conversation_visible_to(resource_uuid, c.actor_id)
        WHEN 'person_question' THEN EXISTS (SELECT 1 FROM person_questions q WHERE q.id = resource_uuid AND q.asked_by_actor_id = c.actor_id)
        ELSE true
    END FROM external_agent_connections c WHERE c.id = connection_uuid;
$$;
