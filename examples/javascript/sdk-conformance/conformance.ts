import {
  ROOT_CONTEXT,
  SpanKind,
  type Attributes,
  type Span,
  type Tracer,
} from '@opentelemetry/api';
import { OTLPTraceExporter } from '@opentelemetry/exporter-trace-otlp-http';
import { resourceFromAttributes } from '@opentelemetry/resources';
import { BatchSpanProcessor } from '@opentelemetry/sdk-trace-base';
import { NodeTracerProvider } from '@opentelemetry/sdk-trace-node';
import * as sideseat from '@sideseat/sdk';
import type { SpanOptions } from '@sideseat/sdk';

const SESSION_ID = 'sdk-conformance-session';
const USER_ID = 'sdk-conformance-user';
const INPUT_MESSAGES =
  '[{"role":"user","parts":[{"type":"text","content":"What is the weather in London?"}]}]';
const FIRST_OUTPUT =
  '[{"role":"assistant","parts":[{"type":"text","content":"I will check the weather."}],"finish_reason":"tool_calls"}]';
const FINAL_OUTPUT =
  '[{"role":"assistant","parts":[{"type":"text","content":"It is 18°C and sunny in London."}],"finish_reason":"stop"}]';

function addFirstModelCall(span: Span): void {
  span.setAttributes({
    'gen_ai.operation.name': 'chat',
    'gen_ai.provider.name': 'conformance',
    'gen_ai.request.model': 'canonical-model',
    'gen_ai.input.messages': INPUT_MESSAGES,
    'gen_ai.output.messages': FIRST_OUTPUT,
    'gen_ai.usage.input_tokens': 12,
    'gen_ai.usage.output_tokens': 6,
  });
}

function addToolCall(span: Span): void {
  span.setAttributes({
    'gen_ai.operation.name': 'execute_tool',
    'gen_ai.tool.name': 'get_weather',
    'gen_ai.tool.call.id': 'call-weather-1',
    'gen_ai.tool.type': 'function',
    'gen_ai.tool.call.arguments': '{"city":"London"}',
    'gen_ai.tool.call.result': '{"temperature_c":18,"condition":"sunny"}',
  });
}

function addFinalModelCall(span: Span): void {
  span.setAttributes({
    'gen_ai.operation.name': 'chat',
    'gen_ai.provider.name': 'conformance',
    'gen_ai.request.model': 'canonical-model',
    'gen_ai.output.messages': FINAL_OUTPUT,
    'gen_ai.response.finish_reasons': '["stop"]',
    'gen_ai.usage.input_tokens': 24,
    'gen_ai.usage.output_tokens': 10,
  });
}

type SpanFactory = (
  name: string,
  write: (span: Span) => void,
  options?: SpanOptions
) => Promise<void>;

async function emitChildren(span: SpanFactory): Promise<void> {
  await span('chat canonical-model', addFirstModelCall, {
    kind: SpanKind.CLIENT,
  });
  await pause();
  await span('execute_tool get_weather', addToolCall);
  await pause();
  await span('chat canonical-model', addFinalModelCall, {
    kind: SpanKind.CLIENT,
  });
}

/**
 * OpenTelemetry JS records a span's start with millisecond precision and its end with microseconds,
 * so a span started within a millisecond of its predecessor's end can appear to start before it. Real
 * operations are separated by far more; without the gap the two modes would differ by clock rounding.
 */
function pause(): Promise<void> {
  return new Promise((resolve) => setTimeout(resolve, 2));
}

async function runWithSideSeat(): Promise<void> {
  await sideseat.init({
    integrations: [],
    serviceName: 'javascript-conformance',
    resourceAttributes: { 'sideseat.framework': 'javascript-conformance' },
    logs: false,
  });
  await sideseat.trace(
    'canonical-agent-run',
    { sessionId: SESSION_ID, userId: USER_ID },
    async () => {
      await emitChildren(async (name, write, options = {}) => {
        await sideseat.span(name, options, async (span) => {
          write(span);
          await operate();
        });
      });
    }
  );
  if (!(await sideseat.shutdown())) {
    throw new Error('SideSeat JavaScript SDK did not flush its spans');
  }
}

/**
 * Every operation takes measurable time, as real model and tool calls do. A tool span that starts and
 * ends on one clock tick gives its call and result one anchor, which the feed reads as a single
 * response - so without this the two modes' feeds would differ by timing rather than by telemetry.
 */
function operate(): Promise<void> {
  return new Promise((resolve) => setTimeout(resolve, 1));
}

async function runWithOpenTelemetry(): Promise<void> {
  const endpoint = (process.env.SIDESEAT_ENDPOINT ?? 'http://127.0.0.1:5388').replace(/\/+$/, '');
  const project = process.env.SIDESEAT_PROJECT_ID ?? 'default';
  const provider = new NodeTracerProvider({
    resource: resourceFromAttributes({
      'service.name': 'javascript-conformance',
      'sideseat.framework': 'javascript-conformance',
    }),
    spanProcessors: [
      new BatchSpanProcessor(
        new OTLPTraceExporter({
          url: `${endpoint}/otel/${project}/v1/traces`,
        })
      ),
    ],
  });
  provider.register();
  const tracer = provider.getTracer('SideSeat.Conformance.Raw');
  const correlation: Attributes = {
    'session.id': SESSION_ID,
    'user.id': USER_ID,
  };

  await tracer.startActiveSpan(
    'canonical-agent-run',
    { attributes: correlation },
    ROOT_CONTEXT,
    async (root) => {
      try {
        await emitChildren(rawSpanFactory(tracer, correlation));
      } finally {
        root.end();
      }
    }
  );

  await provider.forceFlush();
  await provider.shutdown();
}

function rawSpanFactory(tracer: Tracer, correlation: Attributes): SpanFactory {
  return async (name, write, options = {}) => {
    await tracer.startActiveSpan(
      name,
      {
        attributes: { ...correlation, ...options.attributes },
        kind: options.kind,
      },
      async (span) => {
        try {
          write(span);
          await operate();
        } finally {
          span.end();
        }
      }
    );
  };
}

const mode = process.argv[2];
if (mode === 'sdk') {
  await runWithSideSeat();
} else if (mode === 'otel') {
  await runWithOpenTelemetry();
} else if (mode === '--help' || mode === '-h') {
  console.log('usage: conformance.ts <sdk|otel>');
} else {
  console.error('usage: conformance.ts <sdk|otel>');
  process.exitCode = 2;
}
