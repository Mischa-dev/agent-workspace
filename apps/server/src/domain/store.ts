import { randomUUID, createHash } from "node:crypto";
import type { PoolClient } from "pg";
import type { Kind, RecordRow } from "../../../../packages/shared/src/index.js";
export class AppError extends Error {
  constructor(
    public status: number,
    message: string,
  ) {
    super(message);
  }
}
export const assert = (
  condition: unknown,
  message: string,
  status = 400,
): asserts condition => {
  if (!condition) throw new AppError(status, message);
};
export function hash(value: unknown) {
  return createHash("sha256")
    .update(typeof value === "string" ? value : JSON.stringify(value))
    .digest("hex");
}
export class Store {
  rows = new Map<string, RecordRow>();
  changed = new Set<string>();
  constructor(public client: PoolClient) {}
  async load() {
    const r = await this.client.query(
      "SELECT * FROM records ORDER BY created_at,id",
    );
    for (const row of r.rows)
      this.rows.set(row.id, {
        id: row.id,
        kind: row.kind,
        revision: row.revision,
        data: row.data,
        createdAt: row.created_at.toISOString(),
        updatedAt: row.updated_at.toISOString(),
      });
  }
  all<T = any>(kind: Kind) {
    return [...this.rows.values()].filter(
      (r) => r.kind === kind,
    ) as RecordRow<T>[];
  }
  get<T = any>(id: string, kind?: Kind): RecordRow<T> {
    const row = this.rows.get(id);
    if (!row || (kind && row.kind !== kind))
      throw new AppError(404, "Record not found");
    return row as RecordRow<T>;
  }
  add<T extends object>(kind: Kind, data: T, id = randomUUID()): RecordRow<T> {
    const now = new Date().toISOString();
    const row = { id, kind, revision: 1, data, createdAt: now, updatedAt: now };
    this.rows.set(id, row);
    this.changed.add(id);
    return row;
  }
  patch(id: string, patch: Record<string, any>, expected?: number) {
    const row = this.get(id);
    if (expected !== undefined && row.revision !== expected)
      throw new AppError(
        409,
        "This record changed. Reload before saving; your draft has not been discarded.",
      );
    const next = {
      ...row,
      revision: row.revision + 1,
      data: { ...row.data, ...patch },
      updatedAt: new Date().toISOString(),
    };
    this.rows.set(id, next);
    this.changed.add(id);
    return next;
  }
  async commit(type: string) {
    for (const id of this.changed) {
      const r = this.get(id);
      await this.client.query(
        "INSERT INTO records(id,kind,revision,data,created_at,updated_at) VALUES($1,$2,$3,$4,$5,$6) ON CONFLICT(id) DO UPDATE SET revision=excluded.revision,data=excluded.data,updated_at=excluded.updated_at",
        [
          r.id,
          r.kind,
          r.revision,
          JSON.stringify(r.data),
          r.createdAt,
          r.updatedAt,
        ],
      );
    }
    if (!this.changed.size)
      return Number(
        (await this.client.query("SELECT coalesce(max(id),0) id FROM events"))
          .rows[0].id,
      );
    return Number(
      (
        await this.client.query(
          "INSERT INTO events(type,ids) VALUES($1,$2) RETURNING id",
          [type, JSON.stringify([...this.changed])],
        )
      ).rows[0].id,
    );
  }
}
