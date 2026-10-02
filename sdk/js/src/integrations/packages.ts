import { createRequire } from "node:module";
import { readFileSync } from "node:fs";
import { dirname, join } from "node:path";

/** The installed version of `name`, resolved from the application's working directory. */
export function installedVersion(name: string): string | undefined {
  const require = createRequire(join(process.cwd(), "noop.js"));
  try {
    return (require(`${name}/package.json`) as { version?: string }).version;
  } catch {
    // Packages whose `exports` hide package.json: walk up from the entry point instead.
  }
  try {
    let dir = dirname(require.resolve(name));
    for (let depth = 0; depth < 6; depth += 1) {
      try {
        const manifest = JSON.parse(
          readFileSync(join(dir, "package.json"), "utf8"),
        ) as {
          name?: string;
          version?: string;
        };
        if (manifest.name === name) return manifest.version;
      } catch {
        // No manifest at this level.
      }
      dir = dirname(dir);
    }
  } catch {
    // Not installed.
  }
  return undefined;
}

/** The first installed package among `packages`, with its version. */
export function firstInstalled(
  packages: readonly string[],
): [string, string] | undefined {
  for (const name of packages) {
    const version = installedVersion(name);
    if (version !== undefined) return [name, version];
  }
  return undefined;
}
