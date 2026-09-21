import {
  pgTable,
  text,
  integer,
  jsonb,
  timestamp,
  bigserial,
  index,
  boolean,
  primaryKey,
} from "drizzle-orm/pg-core";
export const records = pgTable(
  "records",
  {
    id: text("id").primaryKey(),
    kind: text("kind").notNull(),
    revision: integer("revision").notNull().default(1),
    data: jsonb("data").notNull(),
    createdAt: timestamp("created_at", { withTimezone: true })
      .notNull()
      .defaultNow(),
    updatedAt: timestamp("updated_at", { withTimezone: true })
      .notNull()
      .defaultNow(),
  },
  (t) => [index("records_kind_idx").on(t.kind)],
);
export const events = pgTable("events", {
  id: bigserial("id", { mode: "number" }).primaryKey(),
  type: text("type").notNull(),
  ids: jsonb("ids").notNull(),
  createdAt: timestamp("created_at", { withTimezone: true })
    .notNull()
    .defaultNow(),
});
export const commands = pgTable("commands", {
  id: text("id").primaryKey(),
  actor: text("actor").notNull(),
  digest: text("digest").notNull(),
  result: jsonb("result").notNull(),
  createdAt: timestamp("created_at", { withTimezone: true })
    .notNull()
    .defaultNow(),
});
export const settings = pgTable("settings", {
  key: text("key").primaryKey(),
  value: jsonb("value").notNull(),
});
export const sessions = pgTable("owner_sessions", {
  hash: text("hash").primaryKey(),
  csrf: text("csrf").notNull(),
  expiresAt: timestamp("expires_at", { withTimezone: true }).notNull(),
});
export const connectorCredentials = pgTable("connector_credentials", {
  hash: text("hash").primaryKey(),
  actorId: text("actor_id").notNull(),
  requestId: text("request_id").notNull(),
  expiresAt: timestamp("expires_at", { withTimezone: true }).notNull(),
  revoked: boolean("revoked").notNull().default(false),
});

export const agentCredentials = pgTable(
  "agent_credentials",
  {
    id: text("id").primaryKey(),
    hash: text("hash").notNull().unique(),
    agentId: text("agent_id").notNull(),
    label: text("label").notNull(),
    createdAt: timestamp("created_at", { withTimezone: true })
      .notNull()
      .defaultNow(),
    expiresAt: timestamp("expires_at", { withTimezone: true }).notNull(),
    revokedAt: timestamp("revoked_at", { withTimezone: true }),
    lastSeenAt: timestamp("last_seen_at", { withTimezone: true }),
  },
  (t) => [index("agent_credentials_agent_idx").on(t.agentId)],
);
export const agentConnections = pgTable("agent_connections", {
  agentId: text("agent_id").primaryKey(),
  webhookSecret: text("webhook_secret").notNull(),
});
export const webhookDeliveries = pgTable("webhook_deliveries", {
  id: text("id").primaryKey(),
  requestId: text("request_id").notNull(),
  generation: integer("generation").notNull(),
  attempts: integer("attempts").notNull().default(0),
  nextAttemptAt: timestamp("next_attempt_at", { withTimezone: true })
    .notNull()
    .defaultNow(),
  deliveredAt: timestamp("delivered_at", { withTimezone: true }),
  lastError: text("last_error"),
});
export const oauthState = pgTable(
  "oauth_state",
  {
    kind: text("kind").notNull(),
    id: text("id").notNull(),
    payload: jsonb("payload").notNull(),
    expiresAt: timestamp("expires_at", { withTimezone: true }).notNull(),
  },
  (t) => [
    primaryKey({ columns: [t.kind, t.id] }),
    index("oauth_state_expires_idx").on(t.expiresAt),
  ],
);
