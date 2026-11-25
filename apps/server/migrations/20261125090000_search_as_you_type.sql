-- Search finds what you're still typing, and a task by its key.
--
-- `search_tsquery` is `websearch_to_tsquery` with the last word matched as a
-- prefix, so "batt" finds "battery" in every kind, as the command palette's
-- task search already did. Only a plain last word is a prefix: a query that
-- ends in a phrase (quoted, closed or not), an excluded word (-word), "or",
-- or a stop word is left as it is.
CREATE FUNCTION search_tsquery(config regconfig, query text) RETURNS tsquery
LANGUAGE plpgsql IMMUTABLE PARALLEL SAFE AS $$
DECLARE
    parsed tsquery := websearch_to_tsquery(config, query);
    -- What follows the last space or quote: nothing when the query ends in one.
    last_word text := substring(query from '([^\s"]+)\s*$');
BEGIN
    IF numnode(parsed) = 0
        OR last_word IS NULL
        OR last_word ~ '^-'
        OR lower(last_word) = 'or'
        OR (length(query) - length(replace(query, '"', ''))) % 2 = 1
        OR numnode(websearch_to_tsquery(config, last_word)) = 0
    THEN
        RETURN parsed;
    END IF;
    RETURN (parsed::text || ':*')::tsquery;
EXCEPTION WHEN OTHERS THEN
    RETURN parsed;
END;
$$;
