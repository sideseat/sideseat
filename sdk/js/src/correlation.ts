import { context, createContextKey, type Context } from "@opentelemetry/api";
import type {
  ReadableSpan,
  Span,
  SpanProcessor,
} from "@opentelemetry/sdk-trace-base";

/** The session and user a span belongs to. Unset values are inherited from the enclosing scope. */
export interface Correlation {
  readonly sessionId?: string;
  readonly userId?: string;
}

/** A session scope: every span started inside it belongs to `sessionId` and, if given, `userId`. */
export interface SessionOptions extends Correlation {
  readonly sessionId: string;
}

/**
 * Session and user correlation lives under a private context key, not in W3C baggage: HTTP client
 * instrumentation injects baggage into every outgoing request, which would send end-user identifiers
 * to model providers. A private key follows the same paths inside the process and never leaves it.
 */
const KEY = createContextKey("sideseat.correlation");

export function currentCorrelation(
  ctx: Context = context.active(),
): Correlation {
  return (ctx.getValue(KEY) as Correlation | undefined) ?? {};
}

export function withCorrelation(ctx: Context, update: Correlation): Context {
  const merged = { ...currentCorrelation(ctx) };
  if (update.sessionId !== undefined)
    merged.sessionId = requireText("sessionId", update.sessionId);
  if (update.userId !== undefined)
    merged.userId = requireText("userId", update.userId);
  return ctx.setValue(KEY, Object.freeze(merged));
}

/**
 * Stamps `session.id` and `user.id` on every span started inside a correlation scope, including
 * spans a framework creates. Registered first, so the attributes exist before any other processor
 * or exporter reads the span.
 */
export class CorrelationSpanProcessor implements SpanProcessor {
  onStart(span: Span, parentContext: Context): void {
    const { sessionId, userId } = currentCorrelation(parentContext);
    if (sessionId !== undefined) span.setAttribute("session.id", sessionId);
    if (userId !== undefined) span.setAttribute("user.id", userId);
  }

  onEnd(_span: ReadableSpan): void {}

  async forceFlush(): Promise<void> {}

  async shutdown(): Promise<void> {}
}

function requireText(name: string, value: unknown): string {
  if (typeof value !== "string" || value === "") {
    throw new TypeError(
      `${name} must be a non-empty string, got ${JSON.stringify(value)}`,
    );
  }
  return value;
}
