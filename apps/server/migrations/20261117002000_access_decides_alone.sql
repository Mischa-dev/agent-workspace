-- `access` decides every operation on its own: the old check and the
-- compare mode that ran it beside `access` are gone (one deploy, nothing
-- to move gradually), and so is the log of their differences.
DROP TABLE access_disagreements;
