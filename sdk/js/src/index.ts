import { diag } from "@opentelemetry/api";
import { SideSeat } from "./sideseat.js";
import { SideSeatError } from "./config.js";
import type { SideSeatOptions } from "./config.js";

// Global instance management
let _instance: SideSeat | null = null;
let _initPromise: Promise<SideSeat> | null = null;

/**
 * Initialise SideSeat and return the client.
 *
 * One entry point, and it is async. There used to be two - a synchronous `init()` and an asynchronous
 * `createClient()` - which forced every caller to choose between them with no way to tell which they needed,
 * and made the framework wiring impossible to do correctly: registering the Vercel AI SDK's telemetry
 * integration means importing the user's `ai` package, which cannot be done from a synchronous function, so
 * `experimental_telemetry: { isEnabled: true }` silently produced nothing. Awaiting one call fixes that and
 * removes the choice.
 *
 * Idempotent: a second call returns the same client and warns, and concurrent calls share one initialisation
 * rather than racing.
 */
export async function init(options: SideSeatOptions): Promise<SideSeat> {
  if (_instance !== null) {
    diag.warn("[sideseat] Already initialized; returning existing instance");
    return _instance;
  }
  // Share the in-flight initialisation instead of starting a second one.
  if (_initPromise !== null) {
    return _initPromise;
  }
  _initPromise = SideSeat.create(options)
    .then((client) => {
      _instance = client;
      _initPromise = null;
      return client;
    })
    .catch((err) => {
      _initPromise = null;
      throw err;
    });
  return _initPromise;
}

/**
 * Get the global SideSeat instance.
 * Throws if not initialized.
 */
export function getClient(): SideSeat {
  if (_instance === null) {
    throw new SideSeatError(
      "SideSeat not initialized. Call init() or createClient() first.",
    );
  }
  return _instance;
}

/**
 * Shutdown the global SideSeat instance.
 *
 * Flushes pending spans and releases resources. Resolves to whether every span was exported - `false`
 * means some were lost, which a caller draining before exit needs to know and could not previously
 * learn: this resolved `void` regardless, and the diagnostic warning is silent until the host installs
 * a `diag` logger.
 */
export async function shutdown(): Promise<boolean> {
  // Wait for any pending init to complete first
  if (_initPromise !== null) {
    try {
      await _initPromise;
    } catch (err) {
      diag.debug(`[sideseat] Init error during shutdown: ${err}`);
    }
  }

  if (_instance !== null) {
    const flushed = await _instance.shutdown();
    _instance = null;
    return flushed;
  }
  return true;
}

/**
 * Check if SideSeat has been initialized.
 */
export function isInitialized(): boolean {
  return _instance !== null;
}

// Re-exports
export { SideSeat } from "./sideseat.js";
export {
  Config,
  Frameworks,
  LOG_LEVELS,
  SideSeatError,
  DEFAULT_ENDPOINT,
  DEFAULT_PROJECT_ID,
} from "./config.js";
export type { SideSeatOptions, LogLevel, Framework } from "./config.js";
export { JsonFileSpanExporter, spanToDict, encodeValue } from "./exporters.js";
export { VERSION } from "./version.js";
