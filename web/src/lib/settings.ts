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
