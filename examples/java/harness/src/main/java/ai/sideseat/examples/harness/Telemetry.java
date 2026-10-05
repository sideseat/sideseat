package ai.sideseat.examples.harness;

import io.opentelemetry.api.GlobalOpenTelemetry;
import io.opentelemetry.api.common.AttributeKey;
import io.opentelemetry.api.common.Attributes;
import io.opentelemetry.api.trace.Span;
import io.opentelemetry.api.trace.StatusCode;
import io.opentelemetry.context.Context;
import io.opentelemetry.context.Scope;
import io.opentelemetry.exporter.otlp.http.logs.OtlpHttpLogRecordExporter;
import io.opentelemetry.exporter.otlp.http.trace.OtlpHttpSpanExporter;
import io.opentelemetry.sdk.OpenTelemetrySdk;
import io.opentelemetry.sdk.logs.SdkLoggerProvider;
import io.opentelemetry.sdk.logs.export.BatchLogRecordProcessor;
import io.opentelemetry.sdk.resources.Resource;
import io.opentelemetry.sdk.trace.SdkTracerProvider;
import io.opentelemetry.sdk.trace.export.BatchSpanProcessor;
import java.net.URI;
import java.util.ArrayList;
import java.util.List;
import java.util.concurrent.TimeUnit;

/**
 * The OpenTelemetry pipeline a suite runs under.
 *
 * <p>Native mode is plain OpenTelemetry exporting OTLP, configured the way the framework documents.
 * Recipe mode ({@code --sideseat}) is the same pipeline plus the resource attribute SideSeat's
 * OpenTelemetry recipe adds - {@code sideseat.framework}, naming the integration - since there is no
 * SideSeat SDK for the JVM: comparing the two shows the recipe adds nothing wrong and loses nothing.
 */
public final class Telemetry {
  public final String mode;
  public final String serviceName;
  public final OpenTelemetrySdk sdk;
  private final Attributes resourceAttributes;
  private final List<Runnable> beforeFlush = new ArrayList<>();

  Telemetry(String mode, String serviceName, List<String> integrations) {
    this.mode = mode;
    this.serviceName = serviceName;
    var attributes = Attributes.builder().put("service.name", serviceName);
    if (mode.equals("sdk") && !integrations.isEmpty()) {
      attributes.put("sideseat.framework", integrations.getFirst());
    }
    this.resourceAttributes = attributes.build();
    Resource resource = Resource.create(resourceAttributes);
    String base = otlpBase();
    var traces = OtlpHttpSpanExporter.builder().setEndpoint(base + "/v1/traces");
    var logs = OtlpHttpLogRecordExporter.builder().setEndpoint(base + "/v1/logs");
    String key = System.getenv("SIDESEAT_API_KEY");
    if (key != null && !key.isBlank()) {
      traces.addHeader("Authorization", "Bearer " + key);
      logs.addHeader("Authorization", "Bearer " + key);
    }
    this.sdk = OpenTelemetrySdk.builder()
        .setTracerProvider(SdkTracerProvider.builder().setResource(resource)
            .addSpanProcessor(BatchSpanProcessor.builder(traces.build()).build()).build())
        .setLoggerProvider(SdkLoggerProvider.builder().setResource(resource)
            .addLogRecordProcessor(BatchLogRecordProcessor.builder(logs.build()).build()).build())
        .build();
    GlobalOpenTelemetry.set(sdk);
  }

  /** The resource attributes of this pipeline, for a framework that builds an SDK of its own. */
  public java.util.Map<String, Object> resourceAttributes() {
    java.util.Map<String, Object> out = new java.util.LinkedHashMap<>();
    resourceAttributes.forEach((key, value) -> out.put(key.getKey(), value));
    return out;
  }

  /** A span exporter to this pipeline's endpoint, for a framework that builds an SDK of its own. */
  public io.opentelemetry.sdk.trace.export.SpanExporter spanExporter() {
    var exporter = OtlpHttpSpanExporter.builder().setEndpoint(otlpBase() + "/v1/traces");
    String key = System.getenv("SIDESEAT_API_KEY");
    if (key != null && !key.isBlank()) exporter.addHeader("Authorization", "Bearer " + key);
    return exporter.build();
  }

  /** Where both modes export: the SideSeat project endpoint, or the capture recorder. */
  static String otlpBase() {
    String endpoint = System.getenv().getOrDefault("SIDESEAT_ENDPOINT", "");
    if (endpoint.isBlank()) endpoint = "http://127.0.0.1:5388";
    endpoint = endpoint.replaceAll("/+$", "");
    String path = URI.create(endpoint).getPath();
    if (path != null && !path.isEmpty() && !path.equals("/")) return endpoint;
    String project = System.getenv().getOrDefault("SIDESEAT_PROJECT_ID", "");
    return endpoint + "/otel/" + (project.isBlank() ? "default" : project);
  }

  /** Runs before the providers flush, for a framework that buffers telemetry itself. */
  public void beforeFlush(Runnable action) {
    beforeFlush.add(action);
  }

  /**
   * A root span for one conversation. OpenTelemetry has no session scope, so the root span carries
   * the identifiers, which is what SideSeat's recipe tells users to do.
   */
  <T> T trace(String name, String sessionId, String userId, Sample.Body<T> body) throws Exception {
    Span span = sdk.getTracer("example").spanBuilder(name).setNoParent()
        .setAttribute(AttributeKey.stringKey("session.id"), sessionId)
        .setAttribute(AttributeKey.stringKey("user.id"), userId)
        .startSpan();
    try (Scope ignored = span.makeCurrent()) {
      return body.run();
    } catch (Exception e) {
      span.recordException(e);
      span.setStatus(StatusCode.ERROR, String.valueOf(e.getMessage()));
      throw e;
    } finally {
      span.end();
    }
  }

  void shutdown() {
    beforeFlush.forEach(Runnable::run);
    var traced = sdk.getSdkTracerProvider().forceFlush().join(30, TimeUnit.SECONDS);
    var logged = sdk.getSdkLoggerProvider().forceFlush().join(30, TimeUnit.SECONDS);
    if (!traced.isSuccess() || !logged.isSuccess()) {
      throw new IllegalStateException("the exporter could not export every span and log");
    }
    sdk.close();
  }

  /** The context of the current span, for a framework that takes one explicitly. */
  public static Context current() {
    return Context.current();
  }
}
