/**
 * Web structure guards (P0 plan 3.8): rules counted over `src/`, with
 * allowances in `structure.allowances.json` saying where today's offenders
 * are. A rule fails when the app as a whole has more than its allowances add
 * up to; moving code between files needs no change here. AGENTS.md, "Rules
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
  /** Files the rule doesn't look at (`src/`-relative). */
  skip?: (file: string) => boolean;
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

/** W1.12c took every page off `<AppShell`; the shell itself is the only frame. */
const CEILING_APP_SHELL = 0;

const rules: Rule[] = [
  {
    id: "web_role_name_gates",
    what: "a check on a built-in role name; ask for a permission instead",
    ceiling: 13,
    count: (source) => countAll(squeeze(source), roleNeedles),
  },
  {
    id: "web_account_kind_gates",
    what: "a check on the account kind; ask for a permission instead",
    ceiling: 1,
    count: (source) => countAll(squeeze(source), kindNeedles),
  },
  {
    // W1.2 (guard 6.5): keys come from a feature's key factory (or the op
    // client once a module moves to it), never an array written where it's used.
    id: "web_inline_query_keys",
    what: "a query key written inline; add it to the feature's key factory",
    ceiling: 0,
    count: (source) => squeeze(source).split("queryKey:[").length - 1,
  },
  {
    // W1.12 (guard 6.3): pages tell the persistent shell what they need with
    // useShellPage; they don't wrap themselves in AppShell. W1.12c ends it.
    id: "web_app_shell_in_pages",
    what: "a page wrapped in <AppShell>; call useShellPage and render only the page",
    ceiling: CEILING_APP_SHELL,
    count: (source) => (source.match(/<AppShell[\s>]/g) ?? []).length,
    skip: (file) => file.startsWith("shell/"),
  },
  {
    // W1.14c (AGENTS.md rule 13): a page's address comes from a builder in
    // app/href.ts or app/thingHref.ts (projects/locations.ts until W1.14d),
    // never pasted together from `/projects/${id}` or `/w/${slug}`. API
    // paths (`/api/v1/projects/${id}`) aren't addresses and don't count.
    id: "web_pasted_addresses",
    what: "an address pasted together; build it with spaceHref, workspaceHref, here or thingHref",
    ceiling: 0,
    count: (source) =>
      (source.match(/(?:^|[^\w/])\/(?:projects|w)\/\$\{|['"`]\/(?:projects|w)\/['"`]\s*\+/gm) ?? []).length,
    // The builders themselves, and the one reader that matches addresses
    // to find the thing a pasted link names.
    skip: (file) =>
      ["app/href.ts", "app/thingHref.ts", "projects/locations.ts", "modules/references/thingLinks.ts"].includes(file),
  },
  {
    id: "web_vendor_names",
    what: "a provider or harness name in the app; use the name its manifest gives",
    ceiling: 15,
    count: (source) => countAll(source, vendorNeedles),
  },
];

// A rule holds to the sum of its allowances, not file by file, so code that
// moves between files needs no bookkeeping; it fails only when the whole app
// grows an offender.
function scan(
  rule: Rule,
  files: Record<string, string>,
  allowed = allowances[rule.id] as Record<string, number>,
) {
  const allowedTotal = Object.values(allowed).reduce((sum, n) => sum + n, 0);
  const found: Record<string, number> = {};
  for (const [path, source] of Object.entries(files)) {
    const file = path.replace(/^\.\//, "");
    if (rule.skip?.(file)) continue;
    const count = rule.count(source);
    if (count > 0) found[file] = count;
  }
  const foundTotal = Object.values(found).reduce((sum, n) => sum + n, 0);
  const over: string[] = [];
  const lower: string[] = [];
  if (foundTotal > allowedTotal) {
    const where = Object.entries(found)
      .filter(([file, count]) => count > (allowed[file] ?? 0))
      .map(([file, count]) => `${file} has ${count}`)
      .join(", ");
    over.push(
      `${rule.id}: ${foundTotal} found, ${allowedTotal} allowed (${where}): ${rule.what} ` +
        '(AGENTS.md, "Rules while the foundations are rebuilt")',
    );
  } else if (foundTotal < allowedTotal)
    lower.push(`${rule.id}: ${foundTotal} found, ${allowedTotal} allowed; the allowance can come down`);
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
      expect(total, `${rule.id}: raise a ceiling only with a reason`).toBeLessThanOrEqual(
        rule.ceiling,
      );
    }
  });

  it("catches a new offender and passes a file below its allowance", () => {
    const [roles] = rules;
    expect(
      scan(roles, { "./scratch.tsx": "if (role === 'admin') show()" }, {}).over,
    ).toHaveLength(1);
    expect(
      scan(roles, { "./scratch.tsx": 'if (can("tasks.edit")) show()' }, {}).over,
    ).toEqual([]);
    const keys = rules.find((rule) => rule.id === "web_inline_query_keys")!;
    expect(
      scan(keys, { "./scratch.ts": "useQuery({ queryKey: ['project', id, 'x'] })" }, {}).over,
    ).toHaveLength(1);
    expect(
      scan(keys, { "./scratch.ts": "useQuery({ queryKey: projectKeys.x(id) })" }, {}).over,
    ).toEqual([]);
    const shells = rules.find((rule) => rule.id === "web_app_shell_in_pages")!;
    expect(
      scan(shells, { "./inbox/Scratch.tsx": "return <AppShell home user={user}>{page}</AppShell>" }, {}).over,
    ).toHaveLength(1);
    expect(
      scan(shells, { "./shell/Scratch.tsx": "return <AppShell home user={user}>{page}</AppShell>" }, {}).over,
    ).toEqual([]);
    const kinds = rules[1];
    expect(
      scan(kinds, { "./scratch.ts": "actor.kind !== 'external_agent'" }, {}).over,
    ).toHaveLength(1);
  });
});
