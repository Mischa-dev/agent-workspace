CREATE TABLE agent_credentials (
 id text PRIMARY KEY, hash text NOT NULL UNIQUE, agent_id text NOT NULL,
 label text NOT NULL, created_at timestamptz NOT NULL DEFAULT now(), expires_at timestamptz NOT NULL,
 revoked_at timestamptz, last_seen_at timestamptz
);
--> statement-breakpoint
CREATE INDEX agent_credentials_agent_idx ON agent_credentials(agent_id);
--> statement-breakpoint
CREATE TABLE agent_connections (agent_id text PRIMARY KEY, webhook_secret text NOT NULL);
--> statement-breakpoint
CREATE TABLE webhook_deliveries (
 id text PRIMARY KEY, request_id text NOT NULL, generation integer NOT NULL,
 attempts integer NOT NULL DEFAULT 0, next_attempt_at timestamptz NOT NULL DEFAULT now(),
 delivered_at timestamptz, last_error text
);
--> statement-breakpoint
CREATE TABLE oauth_state (kind text NOT NULL, id text NOT NULL, payload jsonb NOT NULL,
 expires_at timestamptz NOT NULL, PRIMARY KEY(kind,id));
--> statement-breakpoint
CREATE INDEX oauth_state_expires_idx ON oauth_state(expires_at);
