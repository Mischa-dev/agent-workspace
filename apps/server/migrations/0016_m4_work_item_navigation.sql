ALTER TABLE project_navigation_states
    DROP CONSTRAINT project_navigation_states_location_kind_check,
    DROP CONSTRAINT project_navigation_states_location_shape_check;

ALTER TABLE project_navigation_states
    ADD CONSTRAINT project_navigation_states_location_kind_check CHECK (
        location_kind IN ('project_overview', 'channel', 'thread', 'work_items', 'settings')
    ),
    ADD CONSTRAINT project_navigation_states_location_shape_check CHECK (
        (
            location_kind IN ('channel', 'thread')
            AND conversation_id IS NOT NULL
        )
        OR
        (
            location_kind IN ('project_overview', 'work_items', 'settings')
            AND conversation_id IS NULL
        )
    );
