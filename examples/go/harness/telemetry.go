package harness

import (
	"context"
	"fmt"
	"net/url"
	"os"
	"strings"
	"time"

	"go.opentelemetry.io/otel"
	"go.opentelemetry.io/otel/attribute"
	"go.opentelemetry.io/otel/exporters/otlp/otlplog/otlploghttp"
	"go.opentelemetry.io/otel/exporters/otlp/otlptrace/otlptracehttp"
	"go.opentelemetry.io/otel/log/global"
	sdklog "go.opentelemetry.io/otel/sdk/log"
	"go.opentelemetry.io/otel/sdk/resource"
	sdktrace "go.opentelemetry.io/otel/sdk/trace"
	"go.opentelemetry.io/otel/trace"
)

// otlpBase is where both modes export: the SideSeat project endpoint, or the capture recorder.
func otlpBase() string {
	endpoint := strings.TrimRight(os.Getenv("SIDESEAT_ENDPOINT"), "/")
	if endpoint == "" {
		endpoint = "http://127.0.0.1:5388"
	}
	if u, err := url.Parse(endpoint); err == nil && u.Path != "" && u.Path != "/" {
		return endpoint
	}
	project := os.Getenv("SIDESEAT_PROJECT_ID")
	if project == "" {
		project = "default"
	}
	return endpoint + "/otel/" + project
}

func authHeaders() map[string]string {
	if key := os.Getenv("SIDESEAT_API_KEY"); key != "" {
		return map[string]string{"Authorization": "Bearer " + key}
	}
	return nil
}

// Telemetry is the OpenTelemetry pipeline a suite runs under.
//
// Native mode is plain OpenTelemetry exporting OTLP, configured the way the framework documents.
// Recipe mode (`--sideseat`) is the same pipeline plus the resource attribute SideSeat's
// OpenTelemetry recipe adds - `sideseat.framework` naming the integration - since there is no
// SideSeat SDK for Go: comparing the two shows the recipe adds nothing wrong and loses nothing.
type Telemetry struct {
	Mode        string
	ServiceName string
	Tracer      *sdktrace.TracerProvider
	Logger      *sdklog.LoggerProvider
	beforeFlush []func(context.Context)
}

func newTelemetry(mode, serviceName string, integrations []string) (*Telemetry, error) {
	ctx := context.Background()
	attrs := []attribute.KeyValue{attribute.String("service.name", serviceName)}
	if mode == "sdk" && len(integrations) > 0 {
		attrs = append(attrs, attribute.String("sideseat.framework", integrations[0]))
	}
	res := resource.NewSchemaless(attrs...)
	base := otlpBase()
	traces, err := otlptracehttp.New(ctx,
		otlptracehttp.WithEndpointURL(base+"/v1/traces"), otlptracehttp.WithHeaders(authHeaders()))
	if err != nil {
		return nil, err
	}
	logs, err := otlploghttp.New(ctx,
		otlploghttp.WithEndpointURL(base+"/v1/logs"), otlploghttp.WithHeaders(authHeaders()))
	if err != nil {
		return nil, err
	}
	t := &Telemetry{
		Mode:        mode,
		ServiceName: serviceName,
		Tracer:      sdktrace.NewTracerProvider(sdktrace.WithResource(res), sdktrace.WithBatcher(traces)),
		Logger:      sdklog.NewLoggerProvider(sdklog.WithResource(res), sdklog.WithProcessor(sdklog.NewBatchProcessor(logs))),
	}
	otel.SetTracerProvider(t.Tracer)
	global.SetLoggerProvider(t.Logger)
	// Instrumentations record message content only when asked to; every suite asks.
	_ = os.Setenv("OTEL_INSTRUMENTATION_GENAI_CAPTURE_MESSAGE_CONTENT", "true")
	return t, nil
}

// BeforeFlush runs fn before the providers flush, for a framework that buffers telemetry itself.
func (t *Telemetry) BeforeFlush(fn func(context.Context)) { t.beforeFlush = append(t.beforeFlush, fn) }

func (t *Telemetry) shutdown() error {
	ctx, cancel := context.WithTimeout(context.Background(), 30*time.Second)
	defer cancel()
	for _, fn := range t.beforeFlush {
		fn(ctx)
	}
	if err := t.Tracer.ForceFlush(ctx); err != nil {
		return fmt.Errorf("the exporter could not export every span: %w", err)
	}
	if err := t.Logger.ForceFlush(ctx); err != nil {
		return fmt.Errorf("the exporter could not export every log: %w", err)
	}
	_ = t.Tracer.Shutdown(ctx)
	_ = t.Logger.Shutdown(ctx)
	return nil
}

// trace opens a root span for one conversation. OpenTelemetry has no session scope, so the root span
// carries the identifiers, which is what SideSeat's recipe tells users to do.
func (t *Telemetry) trace(ctx context.Context, name, sessionID, userID string, fn func(context.Context) error) error {
	ctx, span := otel.Tracer("example").Start(trace.ContextWithSpanContext(ctx, trace.SpanContext{}), name,
		trace.WithAttributes(attribute.String("session.id", sessionID), attribute.String("user.id", userID)))
	defer span.End()
	if err := fn(ctx); err != nil {
		span.RecordError(err)
		return err
	}
	return nil
}
