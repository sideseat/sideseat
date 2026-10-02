/** Base class for every error the SDK throws. */
export class SideSeatError extends Error {
  constructor(message: string) {
    super(message);
    this.name = new.target.name;
  }
}

/** Settings are invalid, or conflict with the configuration already in effect. */
export class ConfigurationError extends SideSeatError {}

/** An integration that was requested explicitly cannot be installed. */
export class IntegrationError extends SideSeatError {}
