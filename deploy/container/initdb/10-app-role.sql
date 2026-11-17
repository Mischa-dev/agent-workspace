-- Runs once, when the database is first created (the PostgreSQL image's
-- /docker-entrypoint-initdb.d). The server connects as aw_app: it owns the
-- schema but is not a superuser, so row security applies to it. The
-- password comes from the file named by AW_APP_PASSWORD_FILE, read by
-- 11-app-password.sh beside this file.
CREATE ROLE aw_app LOGIN NOSUPERUSER NOCREATEDB NOCREATEROLE NOBYPASSRLS;
ALTER SCHEMA public OWNER TO aw_app;
GRANT CONNECT, TEMPORARY ON DATABASE agent_workspace TO aw_app;
-- The one extension the migrations use, made here by the administrator.
CREATE EXTENSION IF NOT EXISTS pg_trgm WITH SCHEMA public;
