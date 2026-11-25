-- Search finds what you're still typing, and a task by its key.
--
-- `search_tsquery` is `websearch_to_tsquery` with the last word matched as a
-- prefix, so "batt" finds "battery" in every kind, as the command palette's
-- task search already did. A query that ends a phrase (a closing quote), or
-- holds only stop words, is left as it is.
CREATE FUNCTION search_tsquery(config regconfig, query text) RETURNS tsquery
LANGUAGE plpgsql IMMUTABLE PARALLEL SAFE AS $$
DECLARE
    parsed tsquery := websearch_to_tsquery(config, query);
BEGIN
    IF numnode(parsed) = 0 OR query ~ '"\s*$' THEN
        RETURN parsed;
    END IF;
    RETURN (parsed::text || ':*')::tsquery;
EXCEPTION WHEN OTHERS THEN
    RETURN parsed;
END;
$$;
