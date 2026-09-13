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

import { execFileSync } from "node:child_process";
import { existsSync, readFileSync } from "node:fs";
import { createRequire } from "node:module";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";

const root = join(dirname(fileURLToPath(import.meta.url)), "..");

// **Every tracked lockfile**, asked of git rather than listed here. The list was `["web", "sdk/js",
// "examples/javascript", "docs"]` and it matched the tree only by being correct on the day it was written: a
// fifth npm package, or a moved one, would have left the floor derived from fewer lockfiles than exist while
// `--check` certified the stated requirement against that smaller evidence. The floor is what `make setup`
// refuses builds on, so deriving it from a hand-kept list is the one place that inventory must not live.
const packages = execFileSync("git", ["ls-files", "*package-lock.json"], { cwd: root, encoding: "utf8" })
  .split("\n")
  .filter((line) => line.endsWith("package-lock.json") && !line.includes("/node_modules/"))
  .map((line) => dirname(line));
if (packages.length === 0) {
  console.error("No tracked `package-lock.json` found. This script derives the floor from them and cannot guess.");
  process.exit(1);
}

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

// Candidates. A fixed list is not enough for `--check`: it ended at 25, so a claim of "24+" went unverified
// for every later major, and an upper bound like `<26` appearing in some dependency would leave every
// candidate's verdict unchanged while the claim quietly became false. So the probes are **derived from the
// ranges themselves** below - every version any constraint mentions, its neighbours, and a far-future one -
// with this list kept as a floor of familiar release lines.
const seeded = [
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
  // Extra versions may be named on the command line; flags are not versions.
  ...process.argv.slice(2).filter((arg) => !arg.startsWith("-")),
];

// Collect the constraints first, so the probes can be derived from what they actually say.
const constraints = [];
let read = 0;
for (const pkg of packages) {
  const lock = join(root, pkg, "package-lock.json");
  // Tracked but unreadable is a refusal, not a skip: git named it, so a missing file means the working tree
  // and the index disagree, and a floor derived from the rest would be an answer about a different repository.
  if (!existsSync(lock)) {
    console.error(`\ngit tracks ${pkg}/package-lock.json and it is not there. Restore it, or the floor below is measured against fewer packages than the repository has.`);
    process.exit(1);
  }
  read += 1;
  const entries = Object.entries(JSON.parse(readFileSync(lock, "utf8")).packages ?? {});
  for (const [name, meta] of entries) {
    const range = meta?.engines?.node;
    if (!range || meta.optional || meta.os || meta.cpu) continue;
    constraints.push({ who: `${pkg}:${name.replace("node_modules/", "") || "(its own manifest)"}`, range });
  }
}
const ranges = constraints.length;

// Every boundary a constraint mentions becomes a probe, together with its immediate neighbours, so a bound
// nobody thought to sample cannot hide. Plus a far-future version: without it, an upper bound in some
// dependency would leave an open-ended claim like "24+" untested above the largest listed release.
const probes = new Set([...seeded, "999.0.0"]);
for (const { range } of constraints) {
  for (const [, major, minor = "0", patch = "0"] of range.matchAll(/(\d+)(?:\.(\d+))?(?:\.(\d+))?/g)) {
    const [M, m, p] = [Number(major), Number(minor), Number(patch)];
    for (const probe of [
      `${M}.${m}.${p}`,
      `${M}.${m}.${p + 1}`,
      p > 0 ? `${M}.${m}.${p - 1}` : `${M}.${m}.0`,
      `${M}.${m + 1}.0`,
      m > 0 ? `${M}.${m - 1}.0` : `${M}.0.0`,
      `${M + 1}.0.0`,
      M > 0 ? `${M - 1}.0.0` : "0.0.0",
    ]) {
      probes.add(probe);
    }
  }
}
const candidates = [...probes].sort(semver.compare);

const blame = new Map(candidates.map((v) => [v, []]));
for (const { who, range } of constraints) {
  for (const version of candidates) {
    let ok;
    try {
      ok = semver.satisfies(version, range);
    } catch {
      continue; // A range semver cannot parse says nothing.
    }
    if (!ok) blame.get(version).push(`${who} needs ${range}`);
  }
}

console.log(`${ranges} engine range(s) from ${read} lockfile(s) (${packages.join(", ")}), optional and platform-specific excluded\n`);
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
console.log(`\nAccepted: ${accepted.join(", ") || "none"}`);

// `--check` closes the loop. Deriving the answer and printing it leaves the *statement* ungated: a dependency
// bump can raise the floor while CI keeps pinning Node 24, every invariant stays green, and the four places
// that state the requirement quietly become wrong. So the declaration is read back and tested against the
// derivation. `CONTRIBUTING.md` is the declaration because it is what a contributor reads first, and
// `the_node_requirement_is_stated_once` already holds the Makefile's copies identical to it.
if (!process.argv.includes("--check")) {
  console.log(
    "State the resulting range in the same four places: the Makefile header, `make help`, the `setup` " +
      "prerequisite check, and CONTRIBUTING.md. `--check` verifies they still match.",
  );
  process.exit(0);
}

const contributing = readFileSync(join(root, "CONTRIBUTING.md"), "utf8");
const claim = contributing.match(/Node\.js (\d+)\.(\d+)\+ or (\d+)\+/);
if (!claim) {
  console.error(
    "\nCONTRIBUTING.md does not state a Node requirement in the form `Node.js <major>.<minor>+ or <major>+`, " +
      "so there is nothing to check the derivation against.",
  );
  process.exit(1);
}
const [, floorMajor, floorMinor, alsoMajor] = claim.map(Number);
const admits = (version) => {
  const [major, minor] = version.split(".").map(Number);
  return major >= alsoMajor || (major === floorMajor && minor >= floorMinor);
};

const wrong = candidates.filter((v) => admits(v) !== (blame.get(v).length === 0));
if (wrong.length > 0) {
  console.error(
    `\nCONTRIBUTING.md claims ${floorMajor}.${floorMinor}+ or ${alsoMajor}+, which the lockfiles contradict:\n` +
      wrong
        .map((v) =>
          admits(v)
            ? `  ${v} is admitted by the claim but refused by ${blame.get(v)[0]}`
            : `  ${v} is excluded by the claim but every installed range accepts it`,
        )
        .join("\n") +
      "\n\nUpdate the requirement in all four places, then re-run.",
  );
  process.exit(1);
}
console.log(`The stated requirement (${floorMajor}.${floorMinor}+ or ${alsoMajor}+) matches the lockfiles.`);

// A **package-scoped** claim is checked against that package's own lockfile. `examples/javascript/README.md`
// states a floor of its own - lower than the repository's, which is legitimate and useful - and nothing
// verified it: a dependency bump in that one suite raised its floor while the repository-wide check stayed
// green, because the repository floor is the union and the union did not move. A claim nobody checks is a
// convention, which is the same argument that produced `--check` in the first place.
const scopedProblems = [];
for (const pkg of packages) {
  const readme = join(root, pkg, "README.md");
  if (!existsSync(readme)) continue;
  const scoped = readFileSync(readme, "utf8").match(
    /Node\.js\*{0,2}:?\*{0,2}\s*(\d+)\.(\d+)\+ \(within \d+\.x\) or (\d+)\+/,
  );
  if (!scoped) continue;
  const [, major, minor, also] = scoped.map(Number);
  const mine = constraints.filter((c) => c.who.startsWith(`${pkg}:`));
  const admits = (version) => {
    const [M, m] = version.split(".").map(Number);
    return M >= also || (M === major && m >= minor);
  };
  for (const version of candidates) {
    const accepted = mine.every((c) => {
      try {
        return semver.satisfies(version, c.range);
      } catch {
        return true;
      }
    });
    if (admits(version) !== accepted) {
      const why = accepted
        ? "excluded by the claim but every range in this package accepts it"
        : `admitted by the claim but refused by ${mine.find((c) => {
            try {
              return !semver.satisfies(version, c.range);
            } catch {
              return false;
            }
          })?.who}`;
      scopedProblems.push(`  ${pkg}/README.md claims ${major}.${minor}+ or ${also}+: ${version} is ${why}`);
      break;
    }
  }
}
if (scopedProblems.length > 0) {
  console.error(`\nA package states a floor its own lockfile contradicts:\n${scopedProblems.join("\n")}`);
  process.exit(1);
}
console.log(`${packages.length} package(s) checked for a scoped claim of their own.`);
