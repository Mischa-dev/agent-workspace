-- Build 1C (C2.13, C2.19, C2.28) after H1.22: the tables 1C added get the
-- same forced row rule as every other tenant table. Their own migrations ran
-- before `tenancy_protect` existed, so they're protected here. A draft can be
-- written in a DM, so its rule names its conversation.
SELECT tenancy_protect('message_drafts', 'workspace_id', 'conversation_id');
SELECT tenancy_protect('custom_emojis', 'workspace_id');
SELECT tenancy_protect('notification_keywords', 'workspace_id');
