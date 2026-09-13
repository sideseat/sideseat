// Derives the Node versions this repository can be built with, from what the lockfiles actually demand.
//
// The floor was got wrong three times by reasoning about it: "20+" admitted versions that fail, ">=22.12"
// admitted all of 23, and both were written into four places. So it is derived here instead, and the answer
// is printed as a range rather than as a yes/no about whichever Node happens to be running - a check of the
// current version cannot tell you what the floor *is*, which was the defect in the previous recorded form.
//
// Two things this has to be careful about:
//
//   - **Platform-specific and optional packages are excluded.** `@img/sharp-win32-ia32` demands `^20.9.0`
//     and `@napi-rs/lzma-linux-x64-gnu` demands `^22.20 || ^24.12 || >=25`; neither is ever installed here.
//     Intersecting every range in the lockfiles therefore returns the empty set, which is how a naive
//     measurement concludes the repository cannot be built at all.
//   - **`semver` is resolved from an installed tree**, and if none is there the script says so and stops
//     rather than guessing. It is a transitive dependency, not something this repository declares, so its
//     absence is a real possibility and a silent skip would be worse than a refusal.
//
// Usage: node scripts/node-floor.mjs [extra versions to test...]

import { existsSync, readFileSync } from "node:fs";
import { createRequire } from "node:module";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";

const root = join(dirname(fileURLToPath(import.meta.url)), "..");
const packages = ["web", "sdk/js", "examples/javascript", "docs"];

const require = createRequire(import.meta.url);
let semver;
for (const pkg of packages) {
  const candidate = join(root, pkg, "node_modules", "semver");
  if (!existsSync(candidate)) continue;
  try {
    semver = require(candidate);
    break;
  } catch {
    // Try the next tree.
  }
}
if (!semver) {
  console.error(
    "No installed `semver` to compute with. Run `make setup` (or `cd docs && npm ci`) and retry.\n" +
      "It is a transitive dependency rather than one this repository declares, so it is only present " +
      "after an install.",
  );
  process.exit(1);
}

// Candidates: every release line that could plausibly be someone's Node, plus the boundaries the current
// ranges turn on, plus anything named on the command line.
const candidates = [
  "20.18.0",
  "20.19.0",
  "21.7.3",
  "22.11.0",
  "22.12.0",
  "22.13.0",
  "22.20.0",
  "22.21.0",
  "22.22.0",
  "23.0.0",
  "23.11.0",
  "24.0.0",
  "24.12.0",
  "25.0.0",
  "25.2.1",
  ...process.argv.slice(2),
];

const blame = new Map(candidates.map((v) => [v, []]));
let ranges = 0;
for (const pkg of packages) {
  const lock = join(root, pkg, "package-lock.json");
  if (!existsSync(lock)) continue;
  const entries = Object.entries(JSON.parse(readFileSync(lock, "utf8")).packages ?? {});
  for (const [name, meta] of entries) {
    const range = meta?.engines?.node;
    if (!range || meta.optional || meta.os || meta.cpu) continue;
    ranges += 1;
    for (const version of candidates) {
      let ok;
      try {
        ok = semver.satisfies(version, range);
      } catch {
        continue; // A range semver cannot parse says nothing.
      }
      if (!ok) blame.get(version).push(`${pkg}:${name.replace("node_modules/", "")} needs ${range}`);
    }
  }
}

console.log(`${ranges} engine range(s) from ${packages.length} lockfile(s), optional and platform-specific excluded\n`);
const accepted = [];
for (const version of candidates) {
  const refusals = blame.get(version);
  if (refusals.length === 0) {
    accepted.push(version);
    console.log(`  ${version.padEnd(9)} accepted`);
  } else {
    console.log(`  ${version.padEnd(9)} refused by ${refusals.length}, e.g. ${refusals[0]}`);
  }
}
console.log(
  `\nAccepted: ${accepted.join(", ") || "none"}\n` +
    "State the resulting range in the same four places: the Makefile header, `make help`, the `setup` " +
    "prerequisite check, and CONTRIBUTING.md.",
);
