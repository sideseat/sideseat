import { readdirSync, readFileSync, statSync } from "node:fs";
import { join, relative } from "node:path";
import { describe, expect, it } from "vitest";

// @shadcn/lint reads `className`, `cn()`, `cva()` and `tw()`. A class string anywhere else - a lookup
// table, a config object, a function's return value - is invisible to it, which is how dozens of raw
// palette colors outlived the lint. This is the backstop: no source file outside the generated
// primitives names a Tailwind palette color, wherever the string sits.
const SRC = join(import.meta.dirname, "..", "..");
const PALETTE =
  "slate|gray|zinc|neutral|stone|red|orange|amber|yellow|lime|green|emerald|teal|cyan|sky|blue|indigo|violet|purple|fuchsia|pink|rose";
const RAW_COLOR = new RegExp(
  `(?<![\\w-])(?:[a-z-]+:)*(?:text|bg|border(?:-[trblxy])?|ring|ring-offset|outline|fill|stroke|from|via|to|shadow|divide|accent|caret|decoration|placeholder)-(?:${PALETTE})-\\d{2,3}\\b`,
  "g",
);

function sourceFiles(dir: string): string[] {
  return readdirSync(dir).flatMap((name) => {
    const path = join(dir, name);
    if (statSync(path).isDirectory()) {
      if (name === "__tests__" || relative(SRC, path) === join("components", "ui")) return [];
      return sourceFiles(path);
    }
    return /\.(ts|tsx)$/.test(name) && !/\.test\.tsx?$/.test(name) ? [path] : [];
  });
}

describe("design tokens", () => {
  it("no source file uses the raw Tailwind palette", () => {
    const offenders = sourceFiles(SRC).flatMap((file) =>
      [...readFileSync(file, "utf8").matchAll(RAW_COLOR)].map(
        (m) => `${relative(SRC, file)}: ${m[0]}`,
      ),
    );
    expect(offenders).toEqual([]);
  });
});
