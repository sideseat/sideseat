import type { Integration } from "./types.js";

/** Strands Agents for TypeScript emits GenAI spans through the global tracer provider. */
export const strands: Integration = {
  name: "strands",
  packages: ["@strands-agents/sdk"],
  detectable: true,
};
