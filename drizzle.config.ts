import { defineConfig } from "drizzle-kit";
export default defineConfig({
  schema: "./apps/server/src/schema.ts",
  out: "./apps/server/drizzle",
  dialect: "postgresql",
});
