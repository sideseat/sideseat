/**
 * Generic local storage class for managing application settings
 */
export class Settings {
  private static instance: Settings;
  private storageKey: string;

  private constructor(storageKey = "app-settings") {
    this.storageKey = storageKey;
  }

  /**
   * Get singleton instance of Settings
   */
  static getInstance(storageKey = "app-settings"): Settings {
    if (!Settings.instance) {
      Settings.instance = new Settings(storageKey);
    }
    return Settings.instance;
  }

  /**
   * Get a setting value from local storage
   */
  get<T>(key: string, defaultValue?: T): T | null {
    try {
      const data = localStorage.getItem(this.storageKey);
      if (!data) {
        return defaultValue ?? null;
      }

      const settings = JSON.parse(data);
      return settings[key] !== undefined ? settings[key] : (defaultValue ?? null);
    } catch (error) {
      console.error("Error reading from settings:", error);
      return defaultValue ?? null;
    }
  }

  /**
   * Set a setting value in local storage
   */
  set<T>(key: string, value: T): void {
    try {
      const data = localStorage.getItem(this.storageKey);
      const settings = data ? JSON.parse(data) : {};

      settings[key] = value;
      localStorage.setItem(this.storageKey, JSON.stringify(settings));
    } catch (error) {
      console.error("Error writing to settings:", error);
    }
  }

  /**
   * Remove a setting from local storage
   */
  remove(key: string): void {
    try {
      const data = localStorage.getItem(this.storageKey);
      if (!data) return;

      const settings = JSON.parse(data);
      delete settings[key];
      localStorage.setItem(this.storageKey, JSON.stringify(settings));
    } catch (error) {
      console.error("Error removing from settings:", error);
    }
  }

  /**
   * Clear all settings
   */
  clear(): void {
    try {
      localStorage.removeItem(this.storageKey);
    } catch (error) {
      console.error("Error clearing settings:", error);
    }
  }

  /**
   * Get all settings as an object
   */
  getAll(): Record<string, unknown> {
    try {
      const data = localStorage.getItem(this.storageKey);
      return data ? JSON.parse(data) : {};
    } catch (error) {
      console.error("Error reading all settings:", error);
      return {};
    }
  }
}

// Export a default instance
export const settings = Settings.getInstance();

// Global settings keys
export const GLOBAL_PAGE_SIZE_KEY = "global.pageSize";
export const COLOR_SCHEME_KEY = "colorScheme";

// Sidebar settings
export const SIDEBAR_STATE_KEY = "sidebar_state";
export const SIDEBAR_SECTIONS_KEY = "sidebar_sections_state";

// Thread settings
export const MARKDOWN_ENABLED_KEY = "thread.markdownEnabled";

// Home page settings
export const HOME_REALTIME_KEY = "home.realtimeEnabled";

// Traces page settings
export const TRACES_COLUMN_VISIBILITY_KEY = "traces.columnVisibility";
export const TRACES_REALTIME_KEY = "traces.realtimeEnabled";
export const TRACES_SHOW_NON_GENAI_KEY = "traces.showNonGenAi";

// Sessions page settings
export const SESSIONS_COLUMN_VISIBILITY_KEY = "sessions.columnVisibility";
export const SESSIONS_REALTIME_KEY = "sessions.realtimeEnabled";

// Spans page settings
export const SPANS_COLUMN_VISIBILITY_KEY = "spans.columnVisibility";
export const SPANS_REALTIME_KEY = "spans.realtimeEnabled";
export const SPANS_SHOW_NON_GENAI_KEY = "spans.showNonGenAi";

// Trace view settings
export const TRACE_VIEW_SHOW_NON_GENAI_KEY = "traceView.showNonGenAi";

// Trace view settings (per view mode)
const TRACE_VIEW_LAYOUT_PREFIX = "traceView.layout";

/**
 * Generate a settings key for trace view layout per view mode
 */
export function getTraceViewLayoutKey(viewMode: string): string {
  return `${TRACE_VIEW_LAYOUT_PREFIX}.${viewMode}`;
}
