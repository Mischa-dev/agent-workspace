/**
 * Web structure guards (P0 plan 3.8): rules counted per file over `src/`,
 * with allowances in `structure.allowances.json`. A file over its allowance,
 * or one with matches and no entry, fails; a count below it passes and says
 * the allowance can come down. Allowances only shrink: AGENTS.md, "Rules
 * while the foundations are rebuilt", says what to do instead.
 */
import { describe, expect, it } from "vitest";

import allowances from "./structure.allowances.json";

const sources = import.meta.glob<string>(
  ["./**/*.{ts,tsx}", "!./**/*.test.{ts,tsx}", "!./**/test/**"],
  {
    eager: true,
    import: "default",
    query: "?raw",
  },
);

/**
 * The most a rule's allowances may add up to: the total when the guard
 * landed. Lower it as allowances come down; never raise it.
 */
type Rule = {
  id: keyof typeof allowances;
  what: string;
  ceiling: number;
  count: (source: string) => number;
};

/** Whitespace squeezed out, so `role  ===  'admin'` and `role==='admin'` match alike. */
const squeeze = (text: string) => text.replace(/\s+/g, "").replace(/"/g, "'");

const countAll = (text: string, needles: string[]) =>
  needles.reduce((total, needle) => total + text.split(needle).length - 1, 0);

const roleNeedles = ["owner", "admin", "viewer", "member"].flatMap((role) => [
  `role==='${role}'`,
  `role!=='${role}'`,
  `Role==='${role}'`,
  `Role!=='${role}'`,
]);
const kindNeedles = ["external_agent", "human"].flatMap((kind) => [
  `kind==='${kind}'`,
  `kind!=='${kind}'`,
]);
const vendorNeedles = ["Codex", "Claude"];

const rules: Rule[] = [
  {
    id: "web_role_name_gates",
    what: "a check on a built-in role name; ask for a permission instead",
    ceiling: 63,
    count: (source) => countAll(squeeze(source), roleNeedles),
  },
  {
    id: "web_account_kind_gates",
    what: "a check on the account kind; ask for a permission instead",
    ceiling: 10,
    count: (source) => countAll(squeeze(source), kindNeedles),
  },
  {
    id: "web_vendor_names",
    what: "a provider or harness name in the app; use the name its manifest gives",
    ceiling: 20,
    count: (source) => countAll(source, vendorNeedles),
  },
];

function scan(rule: Rule, files: Record<string, string>) {
  const allowed = allowances[rule.id] as Record<string, number>;
  const over: string[] = [];
  const lower: string[] = [];
  for (const [path, source] of Object.entries(files)) {
    const file = path.replace(/^\.\//, "");
    const found = rule.count(source);
    const allowance = allowed[file] ?? 0;
    if (found > allowance)
      over.push(
        `${rule.id}: ${file} has ${found}, allowed ${allowance}: ${rule.what} ` +
          '(AGENTS.md, "Rules while the foundations are rebuilt"; don\'t raise the allowance)',
      );
    else if (found < allowance)
      lower.push(`you can lower ${rule.id} for ${file} to ${found}`);
  }
  return { over, lower };
}

describe("web structure guards", () => {
  it("found the sources", () => {
    expect(Object.keys(sources).length).toBeGreaterThan(100);
  });

  for (const rule of rules) {
    it(`${rule.id} holds to its allowances`, () => {
      const { over, lower } = scan(rule, sources);
      for (const hint of lower) console.info(hint);
      expect(over).toEqual([]);
    });
  }

  it("keeps every rule under its ceiling", () => {
    for (const rule of rules) {
      const total = Object.values(
        allowances[rule.id] as Record<string, number>,
      ).reduce((sum, n) => sum + n, 0);
      expect(total, `${rule.id}: allowances only go down`).toBeLessThanOrEqual(
        rule.ceiling,
      );
    }
  });

  it("catches a new offender and passes a file below its allowance", () => {
    const [roles] = rules;
    expect(
      scan(roles, { "./scratch.tsx": "if (role === 'admin') show()" }).over,
    ).toHaveLength(1);
    expect(
      scan(roles, { "./scratch.tsx": 'if (can("tasks.edit")) show()' }).over,
    ).toEqual([]);
    const kinds = rules[1];
    expect(
      scan(kinds, { "./scratch.ts": "actor.kind !== 'external_agent'" }).over,
    ).toHaveLength(1);
  });
});
