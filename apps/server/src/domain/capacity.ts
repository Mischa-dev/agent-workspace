import type {
  RuntimeCapacityBucket,
  RuntimeCapacitySnapshot,
  RuntimeCapacityWindow,
} from "../connectors/types.js";

export const CAPACITY_MAX_AGE_MS = 90_000;
const MAX_CONFIGURED_AGE_MS = 5 * 60_000;

function record(value: unknown): Record<string, unknown> | null {
  return value !== null && typeof value === "object" && !Array.isArray(value)
    ? (value as Record<string, unknown>)
    : null;
}
function positiveInteger(value: unknown): number | null {
  return typeof value === "number" && Number.isSafeInteger(value) && value > 0
    ? value
    : null;
}
function resetTimestamp(value: unknown): string | null {
  const seconds = positiveInteger(value);
  if (seconds === null || !Number.isSafeInteger(seconds * 1000)) return null;
  const date = new Date(seconds * 1000);
  return Number.isFinite(date.getTime()) ? date.toISOString() : null;
}
function window(
  kind: RuntimeCapacityWindow["kind"],
  value: unknown,
): RuntimeCapacityWindow {
  const raw = record(value);
  const used = raw?.usedPercent;
  const valid =
    typeof used === "number" &&
    Number.isInteger(used) &&
    used >= 0 &&
    used <= 100;
  return {
    kind,
    usedPercent: valid ? used : null,
    remainingPercent: valid ? 100 - used : null,
    durationMinutes: positiveInteger(raw?.windowDurationMins),
    resetsAt: resetTimestamp(raw?.resetsAt),
    validity: valid ? "valid" : value == null ? "unavailable" : "invalid",
  };
}
function bucket(id: string, value: unknown): RuntimeCapacityBucket {
  const raw = record(value);
  return {
    id,
    name:
      typeof raw?.limitName === "string" ? raw.limitName.slice(0, 160) : null,
    windows: [
      window("primary", raw?.primary),
      window("secondary", raw?.secondary),
    ],
  };
}

/** Percentages are consumed capacity. Null is unknown, never zero usage.
 * Reset timestamps are informational and never change the reported usage.
 * Deliberately excludes credits, account identifiers, balances and reset tokens.
 */
export function normalizeCapacity(
  value: unknown,
  observedAt = new Date().toISOString(),
): RuntimeCapacitySnapshot {
  const raw = record(value);
  const map = record(raw?.rateLimitsByLimitId);
  if (map)
    return {
      source: "codex.account/rateLimits/read",
      observedAt,
      format: "multi-bucket",
      buckets: Object.entries(map)
        .slice(0, 100)
        .map(([id, value]) => bucket(id, value)),
    };
  if (raw?.rateLimitsByLimitId != null && !map)
    return {
      source: "codex.account/rateLimits/read",
      observedAt,
      format: "unavailable",
      buckets: [],
      unavailableReason: "missing-telemetry",
    };
  const legacy = record(raw?.rateLimits);
  if (legacy)
    return {
      source: "codex.account/rateLimits/read",
      observedAt,
      format: "legacy",
      buckets: [
        bucket(
          typeof legacy.limitId === "string" && legacy.limitId
            ? legacy.limitId
            : "legacy",
          legacy,
        ),
      ],
    };
  return {
    source: "codex.account/rateLimits/read",
    observedAt,
    format: "unavailable",
    buckets: [],
    unavailableReason: "missing-telemetry",
  };
}

export interface CapacityAssessment {
  status: "available" | "exhausted" | "unknown";
  admission: "allow" | "hold" | "bounded-only";
  fresh: boolean;
  ageMs: number | null;
  applicableBucketIds: string[];
  exhaustedWindows: {
    bucketId: string;
    kind: "primary" | "secondary";
    resetsAt: string | null;
  }[];
  reason: string;
}

/** Capacity is only one admission condition; it never grants execution scope.
 * Unknown telemetry needs the caller's explicit finite time/worker fallback.
 * An old exhausted observation stays held until a new observation replaces it.
 */
export function assessCapacity(
  snapshot: RuntimeCapacitySnapshot | null | undefined,
  options: {
    now?: number;
    maxAgeMs?: number;
    applicableLimitIds?: string[];
    lastKnown?: RuntimeCapacitySnapshot;
  } = {},
): CapacityAssessment {
  const now = options.now ?? Date.now();
  const maxAge =
    Number.isFinite(options.maxAgeMs) && options.maxAgeMs! >= 0
      ? Math.min(options.maxAgeMs!, MAX_CONFIGURED_AGE_MS)
      : CAPACITY_MAX_AGE_MS;
  const observed = snapshot ? Date.parse(snapshot.observedAt) : NaN;
  const validTime =
    Number.isFinite(now) && Number.isFinite(observed) && observed <= now + 5000;
  const ageMs = validTime ? Math.max(0, now - observed) : null;
  const fresh = ageMs !== null && ageMs <= maxAge;
  const applicable = options.applicableLimitIds ?? ["codex"];
  const buckets = (snapshot?.buckets ?? []).filter(
    (value) =>
      applicable.includes(value.id) ||
      (snapshot?.format === "legacy" &&
        value.id === "legacy" &&
        applicable.includes("codex")),
  );
  // Revalidate percentages at the persisted-data boundary too.
  const validWindows = buckets.flatMap((value) =>
    value.windows
      .filter(
        (w) =>
          w.validity === "valid" &&
          Number.isInteger(w.usedPercent) &&
          w.usedPercent! >= 0 &&
          w.usedPercent! <= 100,
      )
      .map((w) => ({ bucketId: value.id, ...w })),
  );
  const exhaustedWindows = validWindows
    .filter((w) => w.usedPercent === 100)
    .map(({ bucketId, kind, resetsAt }) => ({ bucketId, kind, resetsAt }));
  const base = {
    fresh,
    ageMs,
    applicableBucketIds: buckets.map((value) => value.id),
    exhaustedWindows,
  };
  if (exhaustedWindows.length)
    return {
      ...base,
      status: fresh ? "exhausted" : "unknown",
      admission: "hold",
      reason: fresh
        ? "An applicable usage window is fully consumed."
        : "The last applicable observation was exhausted. Refresh telemetry before admitting more work.",
    };
  const prior = options.lastKnown
    ? assessCapacity(options.lastKnown, {
        now,
        maxAgeMs: maxAge,
        applicableLimitIds: applicable,
      })
    : null;
  const unknown = (reason: string): CapacityAssessment => ({
    ...base,
    status: "unknown",
    admission: prior?.admission === "hold" ? "hold" : "bounded-only",
    exhaustedWindows: prior?.admission === "hold" ? prior.exhaustedWindows : [],
    reason:
      prior?.admission === "hold"
        ? "Telemetry did not confirm recovery from the last exhausted observation. Refresh before admitting more work."
        : reason,
  });
  if (!fresh)
    return unknown(
      snapshot
        ? "Usage telemetry is stale or its observation time is invalid."
        : "Usage telemetry is unavailable.",
    );
  // A missing secondary window is unavailable, not a promise of no weekly cap.
  const allApplicablePresent =
    applicable.length > 0 &&
    applicable.every((id) =>
      buckets.some(
        (value) =>
          value.id === id ||
          (id === "codex" &&
            snapshot?.format === "legacy" &&
            value.id === "legacy"),
      ),
    );
  const complete =
    allApplicablePresent &&
    buckets.length > 0 &&
    buckets.every(
      (value) =>
        value.windows.length === 2 &&
        value.windows.every((w) =>
          validWindows.some(
            (v) => v.bucketId === value.id && v.kind === w.kind,
          ),
        ),
    );
  if (!complete)
    return unknown(
      "One or more applicable usage windows are unavailable or invalid.",
    );
  return {
    ...base,
    status: "available",
    admission: "allow",
    reason: "Fresh applicable usage windows report remaining capacity.",
  };
}
