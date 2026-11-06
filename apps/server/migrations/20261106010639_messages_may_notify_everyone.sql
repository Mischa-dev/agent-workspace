-- AWD-74: whether a message may notify everyone in a large conversation
-- (@everyone past the small-audience allowance) is decided when it is sent
-- or edited, while the author's app's agent limit is in force, and stored
-- here: notifications are worked out later, when that limit no longer
-- applies. NULL (older messages, and ones the system or a run posts) falls
-- back to the author's role, as before.

ALTER TABLE messages ADD COLUMN may_notify_everyone boolean;
