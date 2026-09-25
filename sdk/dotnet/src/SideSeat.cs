using System;
using System.Collections.Generic;
using System.Diagnostics;
using System.Threading;
using OpenTelemetry;
using OpenTelemetry.Exporter;
using OpenTelemetry.Resources;
using OpenTelemetry.Trace;

namespace SideSeat;

/// <summary>Configuration for <see cref="SideSeatClient"/>.</summary>
public sealed class SideSeatOptions
{
    /// <summary>Create options for one telemetry producer.</summary>
    /// <param name="framework">
    /// Stable producer identifier such as <c>semantic-kernel</c>, <c>autogen</c>, or <c>custom</c>.
    /// </param>
    public SideSeatOptions(string framework)
    {
        if (string.IsNullOrWhiteSpace(framework))
        {
            throw new ArgumentException("A framework identifier is required.", nameof(framework));
        }

        Framework = framework.Trim();
        Endpoint = ReadEndpoint();
        ProjectId = Read("SIDESEAT_PROJECT_ID") ?? "default";
        ApiKey = Read("SIDESEAT_API_KEY");
        Disabled = ReadBoolean("SIDESEAT_DISABLED");
    }

    /// <summary>Framework or producer name attached to every exported resource.</summary>
    public string Framework { get; }

    /// <summary>
    /// SideSeat base URL, a project OTLP base URL, or a complete <c>/v1/traces</c> URL.
    /// </summary>
    public Uri Endpoint { get; set; }

    /// <summary>Project used when <see cref="Endpoint"/> has no path.</summary>
    public string ProjectId { get; set; }

    /// <summary>Bearer token sent to the OTLP endpoint.</summary>
    public string? ApiKey { get; set; }

    /// <summary>OpenTelemetry <c>service.name</c>.</summary>
    public string? ServiceName { get; set; }

    /// <summary>OpenTelemetry <c>service.version</c>.</summary>
    public string? ServiceVersion { get; set; }

    /// <summary>Disable recording and export without changing application code.</summary>
    public bool Disabled { get; set; }

    /// <summary>Disable the built-in OTLP exporter, for example when a collector is wired separately.</summary>
    public bool ExportTraces { get; set; } = true;

    /// <summary>
    /// Add processors or exporters before the provider is built. This is also the seam used by
    /// conformance tests to inspect completed activities without a network service.
    /// </summary>
    public Action<TracerProviderBuilder>? ConfigureTracerProvider { get; set; }

    private static string? Read(string name)
    {
        var value = Environment.GetEnvironmentVariable(name);
        return string.IsNullOrWhiteSpace(value) ? null : value.Trim();
    }

    private static Uri ReadEndpoint()
    {
        var raw = Read("SIDESEAT_ENDPOINT")
            ?? Read("OTEL_EXPORTER_OTLP_TRACES_ENDPOINT")
            ?? Read("OTEL_EXPORTER_OTLP_ENDPOINT")
            ?? "http://127.0.0.1:5388";
        if (!Uri.TryCreate(raw.TrimEnd('/'), UriKind.Absolute, out var endpoint)
            || (endpoint.Scheme != Uri.UriSchemeHttp && endpoint.Scheme != Uri.UriSchemeHttps))
        {
            throw new ArgumentException(
                $"SIDESEAT_ENDPOINT must be an absolute HTTP(S) URL, got '{raw}'.");
        }

        return endpoint;
    }

    private static bool ReadBoolean(string name)
    {
        var value = Read(name);
        return string.Equals(value, "1", StringComparison.OrdinalIgnoreCase)
            || string.Equals(value, "true", StringComparison.OrdinalIgnoreCase)
            || string.Equals(value, "yes", StringComparison.OrdinalIgnoreCase);
    }
}

/// <summary>
/// A completed or active SideSeat span. Dispose it to end the span and restore the previous
/// session/user context.
/// </summary>
public sealed class SideSeatSpan : IDisposable
{
    private readonly Action? _restoreContext;
    private int _disposed;

    internal SideSeatSpan(Activity? activity, Action? restoreContext)
    {
        Activity = activity;
        _restoreContext = restoreContext;
    }

    /// <summary>The underlying OpenTelemetry activity, or null when telemetry is disabled.</summary>
    public Activity? Activity { get; }

    /// <summary>Add or replace a span attribute.</summary>
    public SideSeatSpan SetAttribute(string name, object? value)
    {
        Activity?.SetTag(name, value);
        return this;
    }

    /// <summary>Record an exception and mark the span as failed.</summary>
    public SideSeatSpan RecordException(Exception exception)
    {
        if (exception == null)
        {
            throw new ArgumentNullException(nameof(exception));
        }

        Activity?.SetStatus(ActivityStatusCode.Error, exception.Message);
        Activity?.AddEvent(new ActivityEvent(
            "exception",
            tags: new ActivityTagsCollection
            {
                { "exception.type", exception.GetType().FullName ?? exception.GetType().Name },
                { "exception.message", exception.Message },
                { "exception.stacktrace", exception.ToString() },
            }));
        return this;
    }

    /// <inheritdoc />
    public void Dispose()
    {
        if (Interlocked.Exchange(ref _disposed, 1) != 0)
        {
            return;
        }

        Activity?.Stop();
        _restoreContext?.Invoke();
    }
}

/// <summary>
/// SideSeat's .NET OpenTelemetry client.
///
/// Use <see cref="StartTrace"/> for an independent root operation and <see cref="StartSpan"/>
/// for work nested under the current activity. Session and user identifiers automatically
/// propagate to child spans created through this client.
/// </summary>
public sealed class SideSeatClient : IDisposable
{
    /// <summary>SDK version.</summary>
    public const string Version = "0.2.0";

    /// <summary>Prefix used by the per-client OpenTelemetry activity source.</summary>
    public const string ActivitySourceName = "SideSeat";

    private static long _sourceSequence;

    private readonly AsyncLocal<CorrelationContext?> _correlation = new AsyncLocal<CorrelationContext?>();
    private readonly ActivitySource? _source;
    private readonly TracerProvider? _provider;
    private int _disposed;

    /// <summary>Create and register a telemetry provider.</summary>
    public SideSeatClient(SideSeatOptions options)
    {
        if (options == null)
        {
            throw new ArgumentNullException(nameof(options));
        }

        Options = options;
        TraceEndpoint = BuildTraceEndpoint(options.Endpoint, options.ProjectId);
        if (options.Disabled)
        {
            return;
        }

        _source = new ActivitySource(
            $"{ActivitySourceName}.{Interlocked.Increment(ref _sourceSequence)}",
            Version);
        var serviceName = string.IsNullOrWhiteSpace(options.ServiceName)
            ? options.Framework
            : options.ServiceName!;
        var resource = ResourceBuilder.CreateDefault()
            .AddService(serviceName, serviceVersion: options.ServiceVersion)
            .AddAttributes(new[]
            {
                new KeyValuePair<string, object>("sideseat.framework", options.Framework),
                new KeyValuePair<string, object>("telemetry.sdk.name", "sideseat"),
                new KeyValuePair<string, object>("telemetry.sdk.language", "dotnet"),
            });

        var builder = Sdk.CreateTracerProviderBuilder()
            .SetResourceBuilder(resource)
            .SetSampler(new AlwaysOnSampler())
            .AddSource(_source.Name);

        if (options.ExportTraces)
        {
            builder.AddOtlpExporter(exporter =>
            {
                exporter.Endpoint = TraceEndpoint;
                exporter.Protocol = OtlpExportProtocol.HttpProtobuf;
                if (!string.IsNullOrWhiteSpace(options.ApiKey))
                {
                    exporter.Headers = $"Authorization=Bearer {options.ApiKey}";
                }
            });
        }

        options.ConfigureTracerProvider?.Invoke(builder);
        _provider = builder.Build();
    }

    /// <summary>The options used to create the client.</summary>
    public SideSeatOptions Options { get; }

    /// <summary>The complete HTTP/protobuf endpoint receiving trace exports.</summary>
    public Uri TraceEndpoint { get; }

    /// <summary>Start a new root trace, detached from any ambient <see cref="Activity.Current"/>.</summary>
    public SideSeatSpan StartTrace(
        string name,
        string? sessionId = null,
        string? userId = null,
        IEnumerable<KeyValuePair<string, object?>>? attributes = null)
    {
        return Start(
            name,
            ActivityKind.Internal,
            default,
            sessionId,
            userId,
            attributes,
            establishContext: true,
            detachFromAmbient: true);
    }

    /// <summary>Start a span under the current activity.</summary>
    public SideSeatSpan StartSpan(
        string name,
        ActivityKind kind = ActivityKind.Internal,
        string? sessionId = null,
        string? userId = null,
        IEnumerable<KeyValuePair<string, object?>>? attributes = null)
    {
        var parent = Activity.Current?.Context ?? default;
        return Start(
            name,
            kind,
            parent,
            sessionId,
            userId,
            attributes,
            establishContext: sessionId != null || userId != null,
            detachFromAmbient: false);
    }

    /// <summary>Export all currently queued spans before the timeout.</summary>
    public bool ForceFlush(int timeoutMilliseconds = 30_000)
    {
        return _provider == null || _provider.ForceFlush(timeoutMilliseconds);
    }

    /// <inheritdoc />
    public void Dispose()
    {
        if (Interlocked.Exchange(ref _disposed, 1) != 0)
        {
            return;
        }

        _provider?.ForceFlush(30_000);
        _provider?.Dispose();
        _source?.Dispose();
    }

    private SideSeatSpan Start(
        string name,
        ActivityKind kind,
        ActivityContext parent,
        string? sessionId,
        string? userId,
        IEnumerable<KeyValuePair<string, object?>>? attributes,
        bool establishContext,
        bool detachFromAmbient)
    {
        if (string.IsNullOrWhiteSpace(name))
        {
            throw new ArgumentException("A span name is required.", nameof(name));
        }

        if (_provider == null || Volatile.Read(ref _disposed) != 0)
        {
            return new SideSeatSpan(null, null);
        }

        var previous = _correlation.Value;
        var current = new CorrelationContext(
            sessionId ?? previous?.SessionId,
            userId ?? previous?.UserId);
        if (establishContext)
        {
            _correlation.Value = current;
        }

        var tags = new ActivityTagsCollection();
        if (!string.IsNullOrWhiteSpace(current.SessionId))
        {
            tags["session.id"] = current.SessionId;
        }
        if (!string.IsNullOrWhiteSpace(current.UserId))
        {
            tags["user.id"] = current.UserId;
        }
        if (attributes != null)
        {
            foreach (var attribute in attributes)
            {
                tags[attribute.Key] = attribute.Value;
            }
        }

        var ambient = Activity.Current;
        if (detachFromAmbient)
        {
            Activity.Current = null;
        }

        Activity? activity;
        try
        {
            activity = _source!.StartActivity(name, kind, parent, tags);
        }
        catch
        {
            if (detachFromAmbient)
            {
                Activity.Current = ambient;
            }
            if (establishContext)
            {
                _correlation.Value = previous;
            }
            throw;
        }

        if (activity == null && detachFromAmbient)
        {
            Activity.Current = ambient;
        }

        Action? restore = establishContext || detachFromAmbient
            ? () =>
            {
                if (establishContext)
                {
                    _correlation.Value = previous;
                }
                if (detachFromAmbient)
                {
                    Activity.Current = ambient;
                }
            }
            : null;
        return new SideSeatSpan(activity, restore);
    }

    private static Uri BuildTraceEndpoint(Uri endpoint, string projectId)
    {
        if (!endpoint.IsAbsoluteUri
            || (endpoint.Scheme != Uri.UriSchemeHttp && endpoint.Scheme != Uri.UriSchemeHttps))
        {
            throw new ArgumentException(
                "The endpoint must be an absolute HTTP(S) URL.",
                nameof(endpoint));
        }

        if (string.IsNullOrWhiteSpace(projectId))
        {
            throw new ArgumentException("A project id is required.", nameof(projectId));
        }

        var builder = new UriBuilder(endpoint)
        {
            Query = string.Empty,
            Fragment = string.Empty,
        };
        var path = builder.Path.TrimEnd('/');
        if (path.EndsWith("/v1/traces", StringComparison.OrdinalIgnoreCase))
        {
            builder.Path = path;
        }
        else if (string.IsNullOrEmpty(path))
        {
            builder.Path = $"/otel/{Uri.EscapeDataString(projectId)}/v1/traces";
        }
        else
        {
            builder.Path = $"{path}/v1/traces";
        }
        return builder.Uri;
    }

    private sealed class CorrelationContext
    {
        internal CorrelationContext(string? sessionId, string? userId)
        {
            SessionId = sessionId;
            UserId = userId;
        }

        internal string? SessionId { get; }
        internal string? UserId { get; }
    }
}
