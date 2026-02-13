import { diag } from "@opentelemetry/api";
import { SideSeat } from "./sideseat.js";
import { SideSeatError } from "./config.js";
import type { SideSeatOptions } from "./config.js";

// Global instance management
let _instance: SideSeat | null = null;
let _initPromise: Promise<SideSeat> | null = null;

/**
 * Initialize SideSeat with the given options.
 * Returns the SideSeat instance synchronously.
 * Use createClient() for async initialization with connection validation.
 */
export function init(options?: SideSeatOptions): SideSeat {
  if (_instance !== null) {
    diag.warn("[sideseat] Already initialized; returning existing instance");
    return _instance;
  }
  if (_initPromise !== null) {
    throw new SideSeatError(
      "Initialization in progress. Use createClient() for async init.",
    );
  }
  _instance = new SideSeat(options);
  return _instance;
}

/**
 * Initialize SideSeat asynchronously with connection validation.
 * Preferred for production use to ensure endpoint is reachable.
 */
export async function createClient(
  options?: SideSeatOptions,
): Promise<SideSeat> {
  if (_instance !== null) {
    diag.warn("[sideseat] Already initialized; returning existing instance");
    return _instance;
  }
  // Return existing promise if init in progress (prevents race)
  if (_initPromise !== null) {
    return _initPromise;
  }
  // Start async init and cache the promise
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
 * Flushes pending spans and releases resources.
 */
export async function shutdown(): Promise<void> {
  // Wait for any pending init to complete first
  if (_initPromise !== null) {
    try {
      await _initPromise;
    } catch (err) {
      diag.debug(`[sideseat] Init error during shutdown: ${err}`);
    }
  }

  if (_instance !== null) {
    await _instance.shutdown();
    _instance = null;
  }
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
