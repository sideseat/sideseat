// Derive the supported Node range from every tracked npm lockfile and verify documented claims.
// Optional and platform-specific packages are excluded because they do not apply to every checkout.
//
// Usage: node scripts/node-floor.mjs [extra versions to test...]

import { execFileSync } from "node:child_process";
import { existsSync, readFileSync } from "node:fs";
import { createRequire } from "node:module";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";

const root = join(dirname(fileURLToPath(import.meta.url)), "..");

// Discover the inventory from Git so adding or moving an npm package cannot bypass the check.
const packages = execFileSync("git", ["ls-files", "*package-lock.json"], {
  cwd: root,
  encoding: "utf8",
})
  .split("\n")
  .filter(
    (line) =>
      line.endsWith("package-lock.json") && !line.includes("/node_modules/"),
  )
  .map((line) => dirname(line));
if (packages.length === 0) {
  console.error(
    "No tracked `package-lock.json` found. This script derives the floor from them and cannot guess.",
  );
  process.exit(1);
}

// The private web package owns repository-wide Node tooling such as Prettier and semver.
const require = createRequire(join(root, "web", "package.json"));
let semver;
try {
  semver = require("semver");
} catch {
  console.error(
    "web/node_modules/semver is not installed. Run `make setup` and retry.",
  );
  process.exit(1);
}

// Seed familiar releases; constraint boundaries and a far-future version are added below.
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

const constraints = [];
let read = 0;
for (const pkg of packages) {
  const lock = join(root, pkg, "package-lock.json");
  // A tracked but missing lockfile would make the result describe only part of the repository.
  if (!existsSync(lock)) {
    console.error(
      `\ngit tracks ${pkg}/package-lock.json and it is not there. Restore it, or the floor below is measured against fewer packages than the repository has.`,
    );
    process.exit(1);
  }
  read += 1;
  const entries = Object.entries(
    JSON.parse(readFileSync(lock, "utf8")).packages ?? {},
  );
  for (const [name, meta] of entries) {
    const range = meta?.engines?.node;
    if (!range || meta.optional || meta.os || meta.cpu) continue;
    constraints.push({
      who: `${pkg}:${name.replace("node_modules/", "") || "(its own manifest)"}`,
      range,
    });
  }
}
const ranges = constraints.length;

// Probe every declared boundary, its neighbours, and an open-ended future major.
const probes = new Set([...seeded, "999.0.0"]);
for (const { range } of constraints) {
  for (const [, major, minor = "0", patch = "0"] of range.matchAll(
    /(\d+)(?:\.(\d+))?(?:\.(\d+))?/g,
  )) {
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

console.log(
  `${ranges} engine range(s) from ${read} lockfile(s) (${packages.join(", ")}), optional and platform-specific excluded\n`,
);
const accepted = [];
for (const version of candidates) {
  const refusals = blame.get(version);
  if (refusals.length === 0) {
    accepted.push(version);
    console.log(`  ${version.padEnd(9)} accepted`);
  } else {
    console.log(
      `  ${version.padEnd(9)} refused by ${refusals.length}, e.g. ${refusals[0]}`,
    );
  }
}
console.log(`\nAccepted: ${accepted.join(", ") || "none"}`);

// `--check` verifies the contributor-facing declaration against the derived range.
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

const wrong = candidates.filter(
  (v) => admits(v) !== (blame.get(v).length === 0),
);
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
console.log(
  `The stated requirement (${floorMajor}.${floorMinor}+ or ${alsoMajor}+) matches the lockfiles.`,
);

// Package README claims may be lower than the repository floor, so validate each against its own constraints.
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
        : `admitted by the claim but refused by ${
            mine.find((c) => {
              try {
                return !semver.satisfies(version, c.range);
              } catch {
                return false;
              }
            })?.who
          }`;
      scopedProblems.push(
        `  ${pkg}/README.md claims ${major}.${minor}+ or ${also}+: ${version} is ${why}`,
      );
      break;
    }
  }
}
if (scopedProblems.length > 0) {
  console.error(
    `\nA package states a floor its own lockfile contradicts:\n${scopedProblems.join("\n")}`,
  );
  process.exit(1);
}
console.log(
  `${packages.length} package(s) checked for a scoped claim of their own.`,
);
