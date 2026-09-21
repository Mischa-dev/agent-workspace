import {
  pgTable,
  text,
  integer,
  jsonb,
  timestamp,
  bigserial,
  index,
  boolean,
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
