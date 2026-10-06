using System;
using System.Collections.Generic;
using System.Diagnostics;
using System.Linq;
using System.Threading;
using Microsoft.Extensions.Logging;
using Microsoft.Extensions.Logging.Abstractions;
using OpenTelemetry;
using OpenTelemetry.Exporter;
using OpenTelemetry.Logs;
using OpenTelemetry.Metrics;
using OpenTelemetry.Resources;
using OpenTelemetry.Trace;

namespace SideSeat;

/// <summary>
/// A configured SideSeat telemetry pipeline: traces, metrics, and logs exported over OTLP/HTTP.
///
/// <code>
/// using var sideseat = SideSeatClient.Create(new SideSeatOptions { Integrations = ["extensions-ai"] });
/// using (sideseat.Session("conversation-42", userId: "user-7"))
/// {
///     await chatClient.GetResponseAsync("Plan a weekend in Lisbon.");
/// }
/// </code>
/// </summary>
public sealed class SideSeatClient : IDisposable
{
    /// <summary>SDK version, recorded as <c>telemetry.sdk.version</c>.</summary>
    public const string Version = "1.0.0";

    /// <summary>The activity source for spans started through this client.</summary>
    internal const string SourceName = "SideSeat";

    private const int DefaultTimeoutMilliseconds = 30_000;

    private static readonly object CreateLock = new object();
    private static SideSeatClient? _current;

    private readonly ActivitySource? _source;
    private readonly TracerProvider? _tracerProvider;
    private readonly MeterProvider? _meterProvider;
    private readonly BatchLogRecordExportProcessor? _logExport;
    private readonly List<Action> _restore = new List<Action>();
    private int _disposed;

    private SideSeatClient(SideSeatSettings settings)
    {
        Settings = settings;
        if (settings.Debug)
        {
            Console.Error.WriteLine(
                $"[sideseat] exporting to {settings.OtlpBase} (export={settings.Export}, disabled={settings.Disabled})");
        }
        if (settings.Disabled)
        {
            return;
        }

        var integrations = SideSeatIntegrations.Resolve(settings);
        SideSeatIntegrations.EnableContentCapture(settings);
        foreach (var (integration, _) in integrations)
        {
            _restore.Add(integration.Enable(settings));
        }
        var resource = SideSeatPipeline.Resource(settings, integrations);
        var sources = SideSeatPipeline.Sources(settings, integrations);

        _source = new ActivitySource(SourceName, Version);
        var tracing = Sdk.CreateTracerProviderBuilder();
        SideSeatPipeline.ConfigureTracing(tracing, settings, resource, sources);
        settings.Configure?.Invoke(tracing);
        _tracerProvider = tracing.Build();

        if (settings.Export && settings.Metrics)
        {
            // Meter providers listen to meters independently, so this one exports beside any the
            // application built.
            var metrics = Sdk.CreateMeterProviderBuilder();
            SideSeatPipeline.ConfigureMetrics(metrics, settings, resource, sources);
            _meterProvider = metrics.Build();
        }

        if (settings.Logs)
        {
            if (settings.Export)
            {
                var exporter = new OtlpExporterOptions();
                SideSeatPipeline.ConfigureExporter(exporter, settings, "logs");
                _logExport = new BatchLogRecordExportProcessor(new OtlpLogExporter(exporter));
            }
            LoggerFactory = Microsoft.Extensions.Logging.LoggerFactory.Create(logging => logging.AddOpenTelemetry(otel =>
            {
                otel.SetResourceBuilder(resource);
                otel.IncludeFormattedMessage = true;
                otel.IncludeScopes = true;
                if (_logExport != null)
                {
                    otel.AddProcessor(_logExport);
                }
            }));
        }

        Integrations = integrations.Select(entry => entry.Integration.Name).ToList();
        AppDomain.CurrentDomain.ProcessExit += OnProcessExit;
        if (settings.Debug)
        {
            Console.Error.WriteLine($"[sideseat] integrations: {string.Join(", ", Integrations)}");
        }
    }

    /// <summary>The client <see cref="Create"/> made for this process, while it is running.</summary>
    public static SideSeatClient? Current => Volatile.Read(ref _current);

    /// <summary>The resolved settings.</summary>
    public SideSeatSettings Settings { get; }

    /// <summary>Names of the installed integrations, primary first.</summary>
    public IReadOnlyList<string> Integrations { get; } = Array.Empty<string>();

    /// <summary>
    /// Loggers whose records are exported to SideSeat with the pipeline's resource. A no-op factory
    /// when the client is disabled or <see cref="SideSeatOptions.Logs"/> is off.
    /// </summary>
    public ILoggerFactory LoggerFactory { get; } = NullLoggerFactory.Instance;

    /// <summary>
    /// Configures telemetry for this process. Calling it again while the client runs returns that
    /// client when the options resolve to the same settings, and throws
    /// <see cref="SideSeatConfigurationException"/> otherwise - the pipeline is process-wide, so a
    /// silent second configuration would leave the process exporting with whichever won. Options
    /// with <see cref="SideSeatOptions.ConfigureTracerProvider"/> never match a running client.
    /// </summary>
    public static SideSeatClient Create(SideSeatOptions? options = null)
    {
        var settings = (options ?? new SideSeatOptions()).Resolve();
        lock (CreateLock)
        {
            if (_current != null)
            {
                if (_current.Settings.Configure == null
                    && settings.Configure == null
                    && _current.Settings.Identity() == settings.Identity())
                {
                    return _current;
                }
                throw new SideSeatConfigurationException(
                    "A SideSeat client with different settings is running; shut it down first.");
            }
            _current = new SideSeatClient(settings);
            return _current;
        }
    }

    /// <summary>Attributes every activity started inside the scope to a session and, optionally, a user.</summary>
    /// <exception cref="ArgumentException">An id is empty.</exception>
    public SideSeatSession Session(string sessionId, string? userId = null) => new SideSeatSession(sessionId, userId);

    /// <summary>
    /// Starts a new root span, even when another activity is current. The session and user apply to
    /// the span and everything started inside it, and are inherited from the current scope when null.
    /// </summary>
    public SideSeatSpan StartTrace(
        string name,
        string? sessionId = null,
        string? userId = null,
        ActivityKind kind = ActivityKind.Internal,
        IEnumerable<KeyValuePair<string, object?>>? attributes = null)
    {
        RequireName(name);
        var ambient = Activity.Current;
        var session = sessionId != null || userId != null ? SideSeatSession.Partial(sessionId, userId) : null;
        Activity.Current = null;
        var activity = StartActivity(name, kind, default, attributes);
        if (activity == null)
        {
            Activity.Current = ambient;
        }
        return new SideSeatSpan(activity, () =>
        {
            session?.Dispose();
            Activity.Current = ambient;
        });
    }

    /// <summary>Starts a child of the current activity, or a root span when there is none.</summary>
    public SideSeatSpan StartSpan(
        string name,
        ActivityKind kind = ActivityKind.Internal,
        IEnumerable<KeyValuePair<string, object?>>? attributes = null)
    {
        RequireName(name);
        return new SideSeatSpan(StartActivity(name, kind, Activity.Current?.Context ?? default, attributes), null);
    }

    /// <summary>Exports everything pending. Returns whether all of it was exported. Never throws.</summary>
    public bool Flush(int timeoutMilliseconds = DefaultTimeoutMilliseconds)
    {
        var traces = _tracerProvider?.ForceFlush(timeoutMilliseconds) ?? true;
        var metrics = _meterProvider?.ForceFlush(timeoutMilliseconds) ?? true;
        var logs = _logExport?.ForceFlush(timeoutMilliseconds) ?? true;
        return traces && metrics && logs;
    }

    /// <summary>
    /// Flushes and stops the pipeline, and restores the switches integrations set. Returns whether
    /// everything was exported. Idempotent; runs automatically when the process exits.
    /// </summary>
    public bool Shutdown(int timeoutMilliseconds = DefaultTimeoutMilliseconds)
    {
        if (Interlocked.Exchange(ref _disposed, 1) != 0)
        {
            return true;
        }
        AppDomain.CurrentDomain.ProcessExit -= OnProcessExit;
        var ok = Flush(timeoutMilliseconds);
        ok = (_tracerProvider?.Shutdown(timeoutMilliseconds) ?? true) && ok;
        ok = (_meterProvider?.Shutdown(timeoutMilliseconds) ?? true) && ok;
        _tracerProvider?.Dispose();
        _meterProvider?.Dispose();
        LoggerFactory.Dispose();
        _source?.Dispose();
        foreach (var restore in Enumerable.Reverse(_restore))
        {
            restore();
        }
        Interlocked.CompareExchange(ref _current, null, this);
        return ok;
    }

    /// <inheritdoc />
    public void Dispose() => Shutdown();

    private void OnProcessExit(object? sender, EventArgs e) => Shutdown();

    private static void RequireName(string name)
    {
        if (string.IsNullOrWhiteSpace(name))
        {
            throw new ArgumentException("A span name is required.", nameof(name));
        }
    }

    private Activity? StartActivity(
        string name,
        ActivityKind kind,
        ActivityContext parent,
        IEnumerable<KeyValuePair<string, object?>>? attributes)
    {
        if (_source == null || Volatile.Read(ref _disposed) != 0)
        {
            return null;
        }
        var tags = new ActivityTagsCollection();
        if (attributes != null)
        {
            foreach (var attribute in attributes)
            {
                tags[attribute.Key] = attribute.Value;
            }
        }
        return _source.StartActivity(name, kind, parent, tags);
    }
}

/// <summary>Adds SideSeat to an OpenTelemetry pipeline the application already hosts.</summary>
public static class SideSeatTracerProviderBuilderExtensions
{
    /// <summary>
    /// Adds SideSeat's correlation, integration sources, resource, and OTLP trace exporter. Use
    /// <see cref="SideSeatClient.Create"/> instead when the application has no pipeline of its own.
    /// Scope sessions with <see cref="SideSeatSession"/>; the application's pipeline owns shutdown.
    /// </summary>
    public static TracerProviderBuilder AddSideSeat(this TracerProviderBuilder builder, SideSeatOptions? options = null)
    {
        var settings = (options ?? new SideSeatOptions()).Resolve();
        if (settings.Disabled)
        {
            return builder;
        }
        var integrations = SideSeatIntegrations.Resolve(settings);
        SideSeatIntegrations.EnableContentCapture(settings);
        foreach (var (integration, _) in integrations)
        {
            integration.Enable(settings);
        }
        SideSeatPipeline.ConfigureTracing(
            builder,
            settings,
            SideSeatPipeline.Resource(settings, integrations),
            SideSeatPipeline.Sources(settings, integrations));
        settings.Configure?.Invoke(builder);
        return builder;
    }
}

/// <summary>Adds SideSeat to a metrics pipeline the application already hosts.</summary>
public static class SideSeatMeterProviderBuilderExtensions
{
    /// <summary>
    /// Adds the integrations' meters, <see cref="SideSeatOptions.Sources"/>, SideSeat's resource,
    /// and an OTLP metric exporter. A built meter provider takes no new reader, so this goes where
    /// the application builds its provider; the application's pipeline owns shutdown.
    /// </summary>
    public static MeterProviderBuilder AddSideSeat(this MeterProviderBuilder builder, SideSeatOptions? options = null)
    {
        var settings = (options ?? new SideSeatOptions()).Resolve();
        if (settings.Disabled || !settings.Metrics)
        {
            return builder;
        }
        var integrations = SideSeatIntegrations.Resolve(settings);
        SideSeatPipeline.ConfigureMetrics(
            builder,
            settings,
            SideSeatPipeline.Resource(settings, integrations),
            SideSeatPipeline.Sources(settings, integrations));
        return builder;
    }
}

/// <summary>The parts of the pipeline the client and the hosting extension share.</summary>
internal static class SideSeatPipeline
{
    internal static ResourceBuilder Resource(
        SideSeatSettings settings,
        IReadOnlyList<(SideSeatIntegration Integration, string Version)> integrations)
    {
        var primary = integrations.Count > 0 ? integrations[0] : default;
        var serviceName = settings.ServiceName ?? primary.Integration?.Package ?? "sideseat-app";
        var serviceVersion = settings.ServiceVersion
            ?? (primary.Integration != null ? primary.Version : SideSeatClient.Version);
        var attributes = new Dictionary<string, object>
        {
            ["telemetry.sdk.name"] = "sideseat",
            ["telemetry.sdk.language"] = "dotnet",
            ["telemetry.sdk.version"] = SideSeatClient.Version,
        };
        if (integrations.Count > 0)
        {
            // The current GenAI conventions are framework-neutral; the server reads this declaration
            // when per-span evidence does not say who produced a span.
            attributes["sideseat.framework"] = integrations[0].Integration.Name;
            attributes["sideseat.integrations"] = integrations.Select(entry => entry.Integration.Name).ToArray();
        }
        foreach (var attribute in settings.ResourceAttributes)
        {
            attributes[attribute.Key] = attribute.Value;
        }
        // CreateDefault reads OTEL_RESOURCE_ATTRIBUTES; what is added after it wins.
        return ResourceBuilder.CreateDefault()
            .AddService(serviceName, serviceVersion: serviceVersion)
            .AddAttributes(attributes);
    }

    internal static IReadOnlyList<string> Sources(
        SideSeatSettings settings,
        IReadOnlyList<(SideSeatIntegration Integration, string Version)> integrations) =>
        integrations.SelectMany(entry => entry.Integration.Sources).Concat(settings.Sources).Distinct().ToList();

    internal static void ConfigureTracing(
        TracerProviderBuilder builder,
        SideSeatSettings settings,
        ResourceBuilder resource,
        IReadOnlyList<string> sources)
    {
        builder
            .SetResourceBuilder(resource)
            .SetSampler(new AlwaysOnSampler())
            .AddProcessor(new CorrelationProcessor())
            .AddSource(SideSeatClient.SourceName);
        foreach (var source in sources)
        {
            builder.AddSource(source);
        }
        if (settings.Export)
        {
            builder.AddOtlpExporter(exporter => ConfigureExporter(exporter, settings, "traces"));
        }
    }

    internal static void ConfigureMetrics(
        MeterProviderBuilder builder,
        SideSeatSettings settings,
        ResourceBuilder resource,
        IReadOnlyList<string> sources)
    {
        builder.SetResourceBuilder(resource);
        foreach (var meter in sources)
        {
            builder.AddMeter(meter);
        }
        if (settings.Export)
        {
            builder.AddOtlpExporter((exporter, _) => ConfigureExporter(exporter, settings, "metrics"));
        }
    }

    internal static void ConfigureExporter(OtlpExporterOptions exporter, SideSeatSettings settings, string signal)
    {
        exporter.Endpoint = settings.SignalEndpoint(signal);
        exporter.Protocol = OtlpExportProtocol.HttpProtobuf;
        var headers = settings.ExportHeaders();
        if (headers.Length > 0)
        {
            exporter.Headers = headers;
        }
    }
}
