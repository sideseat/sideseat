import { describe, it, expect } from "vitest";
import { execFileSync } from "node:child_process";
import { FRAMEWORKS } from "../telemetry-frameworks";

/**
 * The published snippets are the primary onboarding path: a user copies one and runs it.
 * A quoting slip in a Python dict literal shipped three separate times before this test
 * existed, because reading the TypeScript source it lives in does not reveal it — the
 * snippet only breaks once Python parses it. So parse it with Python.
 */

/** Undefined names, via pyflakes when available. Syntax alone misses `llm=llm`. */
function undefinedNames(source: string): string[] {
  try {
    execFileSync("uvx", ["pyflakes", "/dev/stdin"], {
      input: source,
      stdio: ["pipe", "pipe", "pipe"],
    });
    return [];
  } catch (e: unknown) {
    const err = e as { stdout?: Buffer; status?: number };
    const out = err.stdout?.toString() ?? "";
    // status 1 = findings; anything else (tool missing) yields no output and is skipped.
    return out
      .split("\n")
      .filter((l) => l.includes("undefined name"))
      .map((l) => l.split("undefined name")[1].trim());
  }
}

function parsesAsPython(source: string): { ok: boolean; error?: string } {
  try {
    execFileSync("python3", ["-c", "import ast,sys; ast.parse(sys.stdin.read())"], {
      input: source,
      stdio: ["pipe", "pipe", "pipe"],
    });
    return { ok: true };
  } catch (e: unknown) {
    const err = e as { stderr?: Buffer };
    return { ok: false, error: err.stderr?.toString() ?? String(e) };
  }
}

const pythonFrameworks = FRAMEWORKS.filter((f) => f.lang === "python");

describe("telemetry page Python snippets", () => {
  it("has Python frameworks to check", () => {
    expect(pythonFrameworks.length).toBeGreaterThan(10);
  });

  it.each(pythonFrameworks.map((f) => [f.id, f] as const))(
    "%s: SDK snippet is syntactically valid Python",
    (_id, f) => {
      const result = parsesAsPython(f.code());
      expect(result.error ?? "", `${f.id} SDK snippet:\n${f.code()}`).toBe("");
      expect(result.ok).toBe(true);
    },
  );

  it.each(pythonFrameworks.filter((f) => f.altCode).map((f) => [f.id, f] as const))(
    "%s: direct-OTLP snippet is syntactically valid Python",
    (_id, f) => {
      const source = f.altCode!();
      const result = parsesAsPython(source);
      expect(result.error ?? "", `${f.id} altCode snippet:\n${source}`).toBe("");
      expect(result.ok).toBe(true);
    },
  );
});

describe("telemetry page Python snippets - undefined names", () => {
  // pyflakes runs via uvx; if it is unavailable the probe returns nothing and these pass
  // vacuously, so assert the probe itself works first.
  const probeWorks = undefinedNames("x = TotallyUndefined()").length > 0;

  it("the pyflakes probe is actually running", () => {
    expect(probeWorks).toBe(true);
  });

  it.each(pythonFrameworks.map((f) => [f.id, f] as const))(
    "%s: SDK snippet has no undefined names",
    (_id, f) => {
      if (!probeWorks) return;
      expect(undefinedNames(f.code()), `${f.id}:\n${f.code()}`).toEqual([]);
    },
  );
});

describe("telemetry page JavaScript snippets", () => {
  const jsFrameworks = FRAMEWORKS.filter((f) => f.lang === "javascript");

  it("has JavaScript frameworks to check", () => {
    expect(jsFrameworks.length).toBeGreaterThan(2);
  });

  // Balanced-quote check: the same class of slip in a JS/TS snippet.
  it.each(jsFrameworks.map((f) => [f.id, f] as const))(
    "%s: snippet has no unbalanced quotes on a line",
    (_id, f) => {
      const sources = [f.code(), f.altCode?.()].filter(Boolean) as string[];
      for (const source of sources) {
        for (const [i, line] of source.split("\n").entries()) {
          const stripped = line.replace(/\\./g, "");
          for (const q of ['"', "'", "`"]) {
            const count = (stripped.match(new RegExp(q, "g")) ?? []).length;
            expect(count % 2, `${f.id} line ${i + 1} has an odd number of ${q}: ${line}`).toBe(0);
          }
        }
      }
    },
  );
});
