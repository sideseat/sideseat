// Fails on any high or critical advisory in `docs/` that is not one of the known, blocked ones.
//
// `continue-on-error` was the first shape and it tolerates the *next* advisory as much as the current ones -
// so an exemption for a blocked upgrade silently became an exemption for everything. The known set is listed
// here by identifier: a new advisory fails, and one that goes away fails too, because a stale exemption is how
// this list stops meaning anything.
//
// Why these are exempted rather than fixed: the fix is Astro 7.3.2, and the upgrade is blocked upstream by
// `@pasqal-io/starlight-client-mermaid`, whose only two published versions both peer on
// `@astrojs/markdown-remark@^6` while Starlight 0.42 requires `^7.3`. Dropping that plugin means changing how
// the published diagrams render, which is a maintainer's decision rather than a build fix.
//
// Usage: node scripts/audit-docs.mjs   (from the repository root)

import { execFileSync } from "node:child_process";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";

const root = join(dirname(fileURLToPath(import.meta.url)), "..");

// Astro <=7.2.7 and the `sharp`/libvips chain it pins. Each is high or critical; the moderates and lows are
// not listed because the gate is `--audit-level=high`, matching every other npm audit in this repository.
const KNOWN = new Set([
  "GHSA-26w7-cxv4-gfx2", // Astro: RCE through AVIF image optimization
  "GHSA-2883-xcg3-v3hh", // Astro: XSS
  "GHSA-376h-93r7-7g6f", // Astro: XSS
  "GHSA-4g3v-8h47-v7g6", // Astro: reflected XSS via View Transition animation properties
  "GHSA-7pw4-f3q4-r2p2", // Astro: XSS via transition:* directives on hydrated islands
  "GHSA-f48w-9m4c-m7f5", // Astro: XSS via unescaped spread attribute names
  "GHSA-f88m-g3jw-g9cj", // sharp: inherited libvips CVEs
  "GHSA-rgj7-g3m4-5g8c", // sharp/libvips chain
]);

let report;
try {
  report = execFileSync("npm", ["audit", "--json"], {
    cwd: join(root, "docs"),
    encoding: "utf8",
    maxBuffer: 64 * 1024 * 1024,
  });
} catch (error) {
  // `npm audit` exits non-zero when it finds anything, and still prints the report.
  report = error.stdout;
  if (!report) {
    console.error(`npm audit produced no report: ${error.message}`);
    process.exit(1);
  }
}

const found = new Map();
for (const [name, entry] of Object.entries(JSON.parse(report).vulnerabilities ?? {})) {
  if (entry.severity !== "high" && entry.severity !== "critical") continue;
  for (const via of entry.via ?? []) {
    if (typeof via !== "object" || !via.url) continue;
    const id = via.url.split("/").pop();
    found.set(id, `${entry.severity} ${name}: ${via.title ?? via.url}`);
  }
}

const unexpected = [...found].filter(([id]) => !KNOWN.has(id));
const gone = [...KNOWN].filter((id) => !found.has(id));

if (unexpected.length > 0) {
  console.error(`docs: ${unexpected.length} high/critical advisory(ies) that are not the known blocked set:`);
  for (const [id, what] of unexpected) console.error(`  ${id}  ${what}`);
  console.error("\nFix them, or add the identifier to scripts/audit-docs.mjs with the reason.");
}
if (gone.length > 0) {
  console.error(`docs: ${gone.length} exempted advisory(ies) no longer reported - remove them from KNOWN:`);
  for (const id of gone) console.error(`  ${id}`);
}
if (unexpected.length > 0 || gone.length > 0) process.exit(1);

console.log(
  `docs: ${found.size} high/critical advisory(ies), all of them the known set blocked by the Astro upgrade.\n` +
    "See the comment at the top of scripts/audit-docs.mjs.",
);
