using System;
using System.Collections.Generic;
using System.Diagnostics;
using System.Linq;
using System.Threading;
using OpenTelemetry;
using OpenTelemetry.Exporter;
using OpenTelemetry.Resources;
using OpenTelemetry.Trace;

namespace SideSeat;

/// <summary>
/// A configured SideSeat telemetry pipeline.
///
/// <code>
/// using var sideseat = SideSeatClient.Create(new SideSeatOptions { Integrations = { "extensions-ai" } });
/// using (sideseat.Session("conversation-42", userId: "user-7"))
/// {
///     await chatClient.GetResponseAsync("Plan a weekend in Lisbon.");
/// }
/// </code>
/// </summary>
public sealed class SideSeatClient : IDisposable
{
    /// <summary>SDK version.</summary>
    public const string Version = "1.0.0";

    /// <summary>The activity source for spans started through this client.</summary>
    public const string SourceName = "SideSeat";

    private static readonly object CreateLock = new object();
    private static SideSeatClient? _current;

    private readonly ActivitySource? _source;
    private readonly TracerProvider? _provider;
    private int _disposed;

    private SideSeatClient(SideSeatSettings settings)
    {
        Settings = settings;
        if (settings.Disabled)
        {
            return;
        }

        var integrations = settings.Integrations.Select(SideSeatIntegrations.Get).ToList();
        SideSeatIntegrations.EnableContentCapture(settings);
        foreach (var integration in integrations)
        {
            integration.Enable?.Invoke(settings);
        }

        _source = new ActivitySource(SourceName, Version);
        var builder = Sdk.CreateTracerProviderBuilder();
        builder.AddSideSeatPipeline(settings, integrations);
        builder.AddSource(SourceName);
        settings.Configure?.Invoke(builder);
        _provider = builder.Build();
        Integrations = integrations.Select(integration => integration.Name).ToList();
    }

    /// <summary>The client <see cref="Create"/> made for this process, if any.</summary>
    public static SideSeatClient? Current => Volatile.Read(ref _current);

    /// <summary>The resolved settings.</summary>
    public SideSeatSettings Settings { get; }

    /// <summary>Names of the installed integrations, primary first.</summary>
    public IReadOnlyList<string> Integrations { get; } = Array.Empty<string>();

    /// <summary>
    /// Configures telemetry for this process. A second call while a client is alive throws: the
    /// pipeline is process-wide, so a silent second configuration would leave the process exporting
    /// with whichever won.
    /// </summary>
    public static SideSeatClient Create(SideSeatOptions? options = null)
    {
        var settings = (options ?? new SideSeatOptions()).Resolve();
        lock (CreateLock)
        {
            if (_current != null)
            {
                throw new SideSeatConfigurationException(
                    "A SideSeat client already exists for this process; dispose it first.");
            }
            _current = new SideSeatClient(settings);
            return _current;
        }
    }

    /// <summary>Attributes every activity started inside the scope to a session and, optionally, a user.</summary>
    public SideSeatSession Session(string sessionId, string? userId = null)
    {
        if (string.IsNullOrEmpty(sessionId))
        {
            throw new ArgumentException("A session id is required.", nameof(sessionId));
        }
        return new SideSeatSession(sessionId, userId);
    }

    /// <summary>Starts a new root span, even when another activity is current.</summary>
    public SideSeatSpan StartTrace(
        string name,
        string? sessionId = null,
        string? userId = null,
        IEnumerable<KeyValuePair<string, object?>>? attributes = null)
    {
        var ambient = Activity.Current;
        var session = sessionId != null || userId != null ? new SideSeatSession(sessionId, userId) : null;
        Activity.Current = null;
        var activity = StartActivity(name, ActivityKind.Internal, default, attributes);
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

    /// <summary>Starts a child of the current activity.</summary>
    public SideSeatSpan StartSpan(
        string name,
        ActivityKind kind = ActivityKind.Internal,
        IEnumerable<KeyValuePair<string, object?>>? attributes = null)
    {
        return new SideSeatSpan(StartActivity(name, kind, Activity.Current?.Context ?? default, attributes), null);
    }

    /// <summary>Exports everything pending. Returns whether all of it was exported.</summary>
    public bool Flush(int timeoutMilliseconds = 30_000) =>
        _provider == null || _provider.ForceFlush(timeoutMilliseconds);

    /// <summary>Flushes and stops the pipeline. Returns whether every span was exported.</summary>
    public bool Shutdown(int timeoutMilliseconds = 30_000)
    {
        if (Interlocked.Exchange(ref _disposed, 1) != 0)
        {
            return true;
        }
        var ok = Flush(timeoutMilliseconds);
        ok = (_provider?.Shutdown(timeoutMilliseconds) ?? true) && ok;
        _provider?.Dispose();
        _source?.Dispose();
        Interlocked.CompareExchange(ref _current, null, this);
        return ok;
    }

    /// <inheritdoc />
    public void Dispose() => Shutdown();

    private Activity? StartActivity(
        string name,
        ActivityKind kind,
        ActivityContext parent,
        IEnumerable<KeyValuePair<string, object?>>? attributes)
    {
        if (string.IsNullOrWhiteSpace(name))
        {
            throw new ArgumentException("A span name is required.", nameof(name));
        }
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
    /// Adds SideSeat's correlation, integration sources, resource, and OTLP exporter. Use
    /// <see cref="SideSeatClient.Create"/> instead when the application has no pipeline of its own.
    /// </summary>
    public static TracerProviderBuilder AddSideSeat(this TracerProviderBuilder builder, SideSeatOptions? options = null)
    {
        var settings = (options ?? new SideSeatOptions()).Resolve();
        if (settings.Disabled)
        {
            return builder;
        }
        var integrations = settings.Integrations.Select(SideSeatIntegrations.Get).ToList();
        SideSeatIntegrations.EnableContentCapture(settings);
        foreach (var integration in integrations)
        {
            integration.Enable?.Invoke(settings);
        }
        builder.AddSideSeatPipeline(settings, integrations);
        builder.AddSource(SideSeatClient.SourceName);
        settings.Configure?.Invoke(builder);
        return builder;
    }

    internal static void AddSideSeatPipeline(
        this TracerProviderBuilder builder,
        SideSeatSettings settings,
        IReadOnlyList<SideSeatIntegration> integrations)
    {
        var serviceName = settings.ServiceName ?? integrations.FirstOrDefault()?.Name ?? "sideseat-app";
        var attributes = new List<KeyValuePair<string, object>>
        {
            new KeyValuePair<string, object>("telemetry.sdk.name", "sideseat"),
            new KeyValuePair<string, object>("telemetry.sdk.language", "dotnet"),
            new KeyValuePair<string, object>("telemetry.sdk.version", SideSeatClient.Version),
        };
        if (integrations.Count > 0)
        {
            // The current GenAI conventions are framework-neutral; the server reads this declaration
            // when per-span evidence does not say who produced a span.
            attributes.Add(new KeyValuePair<string, object>("sideseat.framework", integrations[0].Name));
            attributes.Add(new KeyValuePair<string, object>(
                "sideseat.integrations",
                integrations.Select(integration => integration.Name).ToArray()));
        }
        builder
            .SetResourceBuilder(ResourceBuilder.CreateDefault()
                .AddService(serviceName, serviceVersion: settings.ServiceVersion ?? SideSeatClient.Version)
                .AddAttributes(attributes))
            .SetSampler(new AlwaysOnSampler())
            .AddProcessor(new CorrelationProcessor());
        foreach (var source in integrations.SelectMany(integration => integration.Sources).Concat(settings.Sources).Distinct())
        {
            builder.AddSource(source);
        }
        if (settings.Export)
        {
            builder.AddOtlpExporter(exporter =>
            {
                exporter.Endpoint = settings.SignalEndpoint("traces");
                exporter.Protocol = OtlpExportProtocol.HttpProtobuf;
                if (!string.IsNullOrWhiteSpace(settings.ApiKey))
                {
                    exporter.Headers = $"Authorization=Bearer {settings.ApiKey}";
                }
            });
        }
    }
}
