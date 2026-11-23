-- C1.27: a page's link can be public: anyone who has it reads that one page
-- without signing in, until it expires or is revoked. A share stays
-- read-only ('see').
ALTER TABLE thing_grants ADD COLUMN public BOOLEAN NOT NULL DEFAULT false;
ALTER TABLE thing_grants
    ADD CONSTRAINT thing_grants_public_is_a_link CHECK (NOT public OR link_digest IS NOT NULL);
