#!/usr/bin/env node
// The web app's packages and their licences (H1.39), from
// `pnpm licenses list --prod`: what ships in the built app, not the tools
// that build it.
//
//   node tools/licenses/web-licenses.mjs --check    fail on a licence not allowed
//   node tools/licenses/web-licenses.mjs --notices  print the notices list
//
// The allowed licences match deny.toml's for Rust, plus the ones only web
// packages use: the fonts' SIL Open Font License and public-domain grants.
// An expression passes when one side of an OR is allowed and every side of
// an AND is.
import { execFileSync } from "node:child_process";

const ALLOWED = new Set([
  "0BSD",
  "Apache-2.0",
  "BSD-2-Clause",
  "BSD-3-Clause",
  "CC0-1.0",
  "ISC",
  "MIT",
  "MPL-2.0",
  "OFL-1.1",
  "Unicode-3.0",
  "Unlicense",
  "Zlib",
]);

// Packages that don't declare a licence pnpm can read, with the one their
// own licence file states. Checked by hand; say which file.
const DECLARED_BY_FILE = {
  khroma: "MIT", // its `license` file is the MIT License
};

function allowed(expression) {
  const text = expression.trim().replace(/^\((.*)\)$/, "$1").trim();
  if (/\sOR\s/.test(text)) return text.split(/\s+OR\s+/).some(allowed);
  if (/\sAND\s/.test(text)) return text.split(/\s+AND\s+/).every(allowed);
  return ALLOWED.has(text.replace(/[()]/g, "").trim());
}

const listed = JSON.parse(
  execFileSync("pnpm", ["licenses", "list", "--json", "--prod"], {
    encoding: "utf8",
    maxBuffer: 64 * 1024 * 1024,
  }),
);
const packages = [];
for (const [licence, entries] of Object.entries(listed)) {
  for (const entry of entries) {
    const declared = licence === "Unknown" ? DECLARED_BY_FILE[entry.name] : licence;
    for (const version of entry.versions) {
      packages.push({ name: entry.name, version, licence: declared ?? "Unknown", homepage: entry.homepage });
    }
  }
}
packages.sort((a, b) => a.name.localeCompare(b.name) || a.version.localeCompare(b.version));

const mode = process.argv[2];
if (mode === "--check") {
  const refused = packages.filter((item) => !allowed(item.licence));
  for (const item of refused) {
    console.error(`web licences: ${item.name}@${item.version} is under ${item.licence}, which isn't allowed`);
  }
  if (refused.length > 0) process.exit(1);
  console.log(`web licences: ${packages.length} packages, every licence allowed`);
} else if (mode === "--notices") {
  console.log("Web app packages (licence as each package declares it)");
  console.log("");
  for (const item of packages) {
    console.log(`${item.name} ${item.version} ${item.licence}${item.homepage ? ` ${item.homepage}` : ""}`);
  }
} else {
  console.error("usage: web-licenses.mjs --check | --notices");
  process.exit(2);
}
