import { describe, it, expect } from "vitest";
import { execFileSync } from "node:child_process";
import { readFileSync, readdirSync } from "node:fs";
import { join, resolve } from "node:path";
import { FRAMEWORKS, withConnection } from "../telemetry-frameworks";

/**
 * The published snippets are the primary onboarding path: a user copies one and runs it.
 * A quoting slip in a Python dict literal shipped three separate times before this test
 * existed, because reading the TypeScript source it lives in does not reveal it — the
 * snippet only breaks once Python parses it. So parse it with Python.
 */

/** The version, pinned: an unpinned `uvx pyflakes` makes the verdict depend on what PyPI serves today. */
const PYFLAKES = "pyflakes==3.4.0";

/**
 * Whether pyflakes can run here.
 *
 * Asked **explicitly**, because the previous form could not tell "no findings" from "no tool": any exit status
 * other than 1 produced no output and was read as a clean result. CI installed only Node, so this check had
 * been passing there without ever running - the shape of gate this repository keeps finding. Absent locally it
 * is a stated skip; absent in CI it is a failure, since CI installs `uv` on purpose.
 */
function pyflakesAvailable(): boolean {
  try {
    execFileSync("uvx", [PYFLAKES, "--version"], { stdio: ["pipe", "pipe", "pipe"] });
    return true;
  } catch {
    if (process.env.CI) {
      throw new Error(
        `uvx ${PYFLAKES} cannot run, and CI installs uv precisely so that it can. ` +
          "Without it this test reports every snippet clean.",
      );
    }
    return false;
  }
}

/** Undefined names, via pyflakes. Syntax alone misses `llm=llm`. */
function undefinedNames(source: string): string[] {
  try {
    execFileSync("uvx", [PYFLAKES, "/dev/stdin"], {
      input: source,
      stdio: ["pipe", "pipe", "pipe"],
    });
    return [];
  } catch (e: unknown) {
    const err = e as { stdout?: Buffer; status?: number };
    // Status 1 is "findings", and availability is established before any of this runs, so any other status is
    // a genuine failure rather than something to read as clean.
    if (err.status !== 1) {
      throw e;
    }
    return (err.stdout?.toString() ?? "")
      .split("\n")
      .filter((l) => l.includes("undefined name"))
      .map((l) => l.split("undefined name")[1].trim());
  }
}

/**
 * The interpreter the **SDK's own floor** names, fetched by uv rather than whatever `python3` the machine has.
 *
 * A snippet that parses on 3.13 and not on 3.11 is broken for a user the SDK says it supports, and a bare
 * `python3` cannot see that: it was the runner's version in CI and the developer's locally, so the verdict
 * moved with the machine. `requires-python = ">=3.11"` in `sdk/python/pyproject.toml` is the number this
 * follows; uv downloads that interpreter if it is not present.
 */
const PYTHON_FLOOR = "3.11";

function parsesAsPython(source: string): { ok: boolean; error?: string } {
  try {
    execFileSync(
      "uv",
      [
        "run",
        "--python",
        PYTHON_FLOOR,
        "--no-project",
        "python",
        "-c",
        "import ast,sys; ast.parse(sys.stdin.read())",
      ],
      {
        input: source,
        stdio: ["pipe", "pipe", "pipe"],
      },
    );
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
  // Two separate questions, because they have different answers: can the tool run at all, and does it report
  // what it is being trusted to report. Asking only the second (the probe returning nothing) conflated a
  // missing tool with a clean result, and the message a reader got was "the probe is not running" for both.
  const available = pyflakesAvailable();

  it("pyflakes can run here", () => {
    expect(available, "uvx pyflakes cannot run - install uv, or see the CI job that does").toBe(
      true,
    );
  });

  it("the pyflakes probe reports a name it should", () => {
    if (!available) return;
    expect(undefinedNames("x = TotallyUndefined()")).not.toEqual([]);
  });

  it.each(pythonFrameworks.map((f) => [f.id, f] as const))(
    "%s: SDK snippet has no undefined names",
    (_id, f) => {
      if (!available) return;
      expect(undefinedNames(f.code()), `${f.id}:\n${f.code()}`).toEqual([]);
    },
  );
});

describe("Vertex AI onboarding contract", () => {
  const vertex = FRAMEWORKS.find((framework) => framework.id === "vertex-ai");

  it("uses the current Google Gen AI Enterprise client in both setup paths", () => {
    expect(vertex).toBeDefined();
    expect(vertex?.install).toBe('pip install "sideseat[vertex-ai]"');
    expect(vertex?.code()).toContain("from google import genai");
    expect(vertex?.code()).toContain("enterprise=True");
    expect(vertex?.code()).not.toContain("import vertexai");
    expect(vertex?.altInstall).toContain("logfire[google-genai]");
    expect(vertex?.altCode?.()).toContain("logfire.instrument_google_genai()");
    expect(vertex?.altCode?.()).not.toContain("VertexAIInstrumentor");
  });
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

/**
 * The integration names each SDK registers, read from the SDK sources so a snippet cannot name an
 * integration the SDK does not have.
 */
// The test environment is jsdom, whose import.meta.url is not a file URL.
const SDK_ROOT = resolve(__dirname, "../../../../../sdk");

function pythonIntegrationNames(): Set<string> {
  const source = readFileSync(
    join(SDK_ROOT, "python/src/sideseat/integrations/__init__.py"),
    "utf8",
  );
  const registry = source.slice(source.indexOf("_REGISTRY"), source.indexOf("\n}\n"));
  return new Set([...registry.matchAll(/^\s+"([a-z0-9-]+)": \(/gm)].map((m) => m[1]));
}

function typescriptIntegrationNames(): Set<string> {
  const dir = join(SDK_ROOT, "js/src/integrations");
  const names = readdirSync(dir)
    .filter((file) => file.endsWith(".ts"))
    .flatMap((file) => [
      ...readFileSync(join(dir, file), "utf8").matchAll(/^\s+name: "([a-z0-9-]+)",$/gm),
    ])
    .map((m) => m[1]);
  return new Set(names);
}

function requestedIntegrations(code: string): string[] {
  const call = /sideseat\.init\((?:\{\s*)?integrations(?:=|: )\[([^\]]*)\]/.exec(code);
  return call ? [...call[1].matchAll(/["']([^"']+)["']/g)].map((m) => m[1]) : [];
}

describe("telemetry page SDK snippets", () => {
  const python = pythonIntegrationNames();
  const typescript = typescriptIntegrationNames();

  it("reads both integration registries", () => {
    expect(python.has("strands")).toBe(true);
    expect(typescript.has("vercel-ai")).toBe(true);
  });

  it.each(FRAMEWORKS.map((f) => [f.id, f] as const))(
    "%s: names an integration the SDK registers",
    (_id, f) => {
      const requested = requestedIntegrations(f.code());
      expect(requested, `${f.id} has no sideseat.init integrations call`).not.toEqual([]);
      const known = f.lang === "python" ? python : typescript;
      for (const name of requested) {
        expect(known.has(name), `${f.id} requests unknown integration ${name}`).toBe(true);
      }
    },
  );

  it.each(FRAMEWORKS.map((f) => [f.id, f] as const))("%s: uses no removed SDK API", (_id, f) => {
    for (const source of [f.code(), f.altCode?.() ?? ""]) {
      expect(source).not.toMatch(
        /SideSeat\(|Frameworks\.|from sideseat import|\binit\(\{ framework/,
      );
    }
  });
});

describe("withConnection", () => {
  const python = 'import sideseat\n\nsideseat.init(integrations=["strands"])\n';
  const typescript =
    "import * as sideseat from '@sideseat/sdk';\n\nawait sideseat.init({ integrations: ['strands'] });\n";

  it("leaves the default project without a key unchanged", () => {
    expect(withConnection(python, "python", { useApiKey: false, projectId: "default" })).toBe(
      python,
    );
    expect(
      withConnection(typescript, "javascript", { useApiKey: false, projectId: "default" }),
    ).toBe(typescript);
  });

  it("adds the project and key to a Python init call", () => {
    expect(withConnection(python, "python", { useApiKey: true, projectId: "team-a" })).toBe(
      'import os\nimport sideseat\n\nsideseat.init(integrations=["strands"], project="team-a", api_key=os.environ["SIDESEAT_API_KEY"])\n',
    );
  });

  it("adds the project and key to a TypeScript init call", () => {
    expect(withConnection(typescript, "javascript", { useApiKey: true, projectId: "team-a" })).toBe(
      "import * as sideseat from '@sideseat/sdk';\n\nawait sideseat.init({ integrations: ['strands'], project: \"team-a\", apiKey: process.env.SIDESEAT_API_KEY });\n",
    );
  });

  it("produces valid Python for every snippet", () => {
    for (const f of pythonFrameworks) {
      const source = withConnection(f.code(), "python", { useApiKey: true, projectId: "team-a" });
      expect(source, f.id).toContain('project="team-a"');
      expect(parsesAsPython(source).error ?? "", `${f.id}:\n${source}`).toBe("");
    }
  });
});
