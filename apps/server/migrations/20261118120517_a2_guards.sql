-- A2.27: a team's own guards. A guard watches the changes accounts make
-- (any operation that writes) in a workspace, or one space of it: those
-- of the operations it names, on the kinds of thing it names, whose text
-- has one of its terms, by the accounts it names. Each part left empty
-- matches everything. It either stops the change (`deny`) or holds it for
-- the accounts it names to review (`review`): the caller is told the
-- change is pending review with the request it raised, and once allowed
-- makes the same call again to carry it out.

CREATE TABLE guards (
    id UUID PRIMARY KEY,
    workspace_id UUID NOT NULL REFERENCES organizations(id) ON DELETE CASCADE,
    -- NULL: every space of the workspace.
    space_id UUID REFERENCES projects(id) ON DELETE CASCADE,
    name TEXT NOT NULL CHECK (char_length(name) BETWEEN 1 AND 120),
    action TEXT NOT NULL CHECK (action IN ('deny', 'review')),
    operations TEXT[] NOT NULL DEFAULT '{}' CHECK (cardinality(operations) <= 50),
    target_kinds TEXT[] NOT NULL DEFAULT '{}' CHECK (cardinality(target_kinds) <= 20),
    terms TEXT[] NOT NULL DEFAULT '{}' CHECK (cardinality(terms) <= 50),
    accounts UUID[] NOT NULL DEFAULT '{}' CHECK (cardinality(accounts) <= 50),
    reviewers UUID[] NOT NULL DEFAULT '{}' CHECK (cardinality(reviewers) <= 20),
    enabled BOOLEAN NOT NULL DEFAULT true,
    version BIGINT NOT NULL DEFAULT 1,
    created_by_account_id UUID NOT NULL REFERENCES actors(id),
    updated_by_account_id UUID NOT NULL REFERENCES actors(id),
    created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    CHECK (action <> 'review' OR cardinality(reviewers) > 0)
);
CREATE INDEX guards_workspace ON guards (workspace_id) WHERE enabled;
SELECT tenancy_protect('guards', 'workspace_id');

-- A change a review guard held: which call it was (by the hash of its
-- input, never the input itself, which may carry a secret) and the request
-- that asks the reviewers.
CREATE TABLE guard_holds (
    id UUID PRIMARY KEY,
    workspace_id UUID NOT NULL REFERENCES organizations(id) ON DELETE CASCADE,
    space_id UUID REFERENCES projects(id) ON DELETE CASCADE,
    guard_id UUID NOT NULL REFERENCES guards(id) ON DELETE CASCADE,
    account_id UUID NOT NULL REFERENCES actors(id),
    operation TEXT NOT NULL,
    input_hash TEXT NOT NULL,
    request_id UUID NOT NULL REFERENCES requests(id) ON DELETE CASCADE,
    state TEXT NOT NULL DEFAULT 'pending'
        CHECK (state IN ('pending', 'allowed', 'declined', 'used')),
    created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at TIMESTAMPTZ NOT NULL DEFAULT now()
);
-- One open hold per call, guard and place: a retry finds the review
-- already asked, and the same call elsewhere is reviewed there.
CREATE UNIQUE INDEX guard_holds_open
    ON guard_holds (workspace_id, guard_id, account_id, operation, input_hash,
                    COALESCE(space_id, '00000000-0000-0000-0000-000000000000'::uuid))
    WHERE state IN ('pending', 'allowed');
CREATE INDEX guard_holds_request ON guard_holds (request_id);
SELECT tenancy_protect('guard_holds', 'workspace_id');
