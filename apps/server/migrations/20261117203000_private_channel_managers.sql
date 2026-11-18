-- C2.6: private channels. A channel already could be visibility 'members'
-- (`conversation_member_visible` reads it); a private channel now gets made
-- that way, its creator its first member. Its managers, and the space's
-- moderators, change who's in it; any member can leave.
ALTER TABLE conversation_members ADD COLUMN manager BOOLEAN NOT NULL DEFAULT false;
