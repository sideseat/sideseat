using System;
using System.Collections.Generic;
using System.Linq;
using OpenTelemetry.Trace;

namespace SideSeat;

/// <summary>
/// Configuration for <see cref="SideSeatClient"/>. Every property falls back to its environment
/// variable, then to a default, when <see cref="Resolve"/> runs.
/// </summary>
public sealed class SideSeatOptions
{
    /// <summary>SideSeat server URL, or an OTLP base URL that already has a path. <c>SIDESEAT_ENDPOINT</c>.</summary>
    public string? Endpoint { get; set; }

    /// <summary>Project that receives the telemetry. <c>SIDESEAT_PROJECT_ID</c>.</summary>
    public string? Project { get; set; }

    /// <summary>Sent as a bearer token. <c>SIDESEAT_API_KEY</c>.</summary>
    public string? ApiKey { get; set; }

    /// <summary><c>service.name</c>. <c>OTEL_SERVICE_NAME</c>; defaults to the primary integration's name.</summary>
    public string? ServiceName { get; set; }

    /// <summary><c>service.version</c>. <c>OTEL_SERVICE_VERSION</c>.</summary>
    public string? ServiceVersion { get; set; }

    /// <summary>
    /// Integration names; the first is the primary one. <c>SIDESEAT_INTEGRATIONS</c>. Known names are
    /// listed in <see cref="SideSeatIntegrations.Names"/>.
    /// </summary>
    public IList<string> Integrations { get; } = new List<string>();

    /// <summary>Additional activity sources to export, such as the application's own.</summary>
    public IList<string> Sources { get; } = new List<string>();

    /// <summary>Record prompts, responses, and tool payloads. <c>SIDESEAT_CAPTURE_CONTENT</c>; on by default.</summary>
    public bool? CaptureContent { get; set; }

    /// <summary>Configure nothing; every call becomes a no-op. <c>SIDESEAT_DISABLED</c>.</summary>
    public bool? Disabled { get; set; }

    /// <summary>Send spans over OTLP. Off is useful with <see cref="ConfigureTracerProvider"/> in tests.</summary>
    public bool Export { get; set; } = true;

    /// <summary>Add processors or exporters before the provider is built.</summary>
    public Action<TracerProviderBuilder>? ConfigureTracerProvider { get; set; }

    /// <summary>Resolves these options against the environment.</summary>
    public SideSeatSettings Resolve()
    {
        var endpoint = ParseEndpoint(
            Endpoint ?? Env("SIDESEAT_ENDPOINT") ?? Env("OTEL_EXPORTER_OTLP_ENDPOINT") ?? SideSeatSettings.DefaultEndpoint);
        var integrations = Integrations.Count > 0
            ? Integrations.ToList()
            : (Env("SIDESEAT_INTEGRATIONS") ?? string.Empty)
                .Split(',')
                .Select(name => name.Trim())
                .Where(name => name.Length > 0)
                .ToList();
        return new SideSeatSettings(
            endpoint,
            (Project ?? Env("SIDESEAT_PROJECT_ID") ?? SideSeatSettings.DefaultProject).Trim(),
            ApiKey ?? Env("SIDESEAT_API_KEY"),
            ServiceName ?? Env("OTEL_SERVICE_NAME"),
            ServiceVersion ?? Env("OTEL_SERVICE_VERSION"),
            integrations,
            Sources.ToList(),
            CaptureContent ?? Flag("SIDESEAT_CAPTURE_CONTENT", true),
            Disabled ?? Flag("SIDESEAT_DISABLED", false),
            Export,
            ConfigureTracerProvider);
    }

    private static string? Env(string name)
    {
        var value = Environment.GetEnvironmentVariable(name);
        return string.IsNullOrWhiteSpace(value) ? null : value.Trim();
    }

    private static bool Flag(string name, bool fallback)
    {
        var raw = Env(name);
        if (raw == null)
        {
            return fallback;
        }
        switch (raw.ToLowerInvariant())
        {
            case "1":
            case "true":
            case "yes":
                return true;
            case "0":
            case "false":
            case "no":
                return false;
            default:
                throw new SideSeatConfigurationException(
                    $"{name} must be one of 1/0, true/false, yes/no; got '{raw}'.");
        }
    }

    private static Uri ParseEndpoint(string raw)
    {
        if (!Uri.TryCreate(raw.Trim().TrimEnd('/'), UriKind.Absolute, out var endpoint)
            || (endpoint.Scheme != Uri.UriSchemeHttp && endpoint.Scheme != Uri.UriSchemeHttps)
            || string.IsNullOrEmpty(endpoint.Host))
        {
            throw new SideSeatConfigurationException($"The endpoint must be an http(s) URL, got '{raw}'.");
        }
        return endpoint;
    }
}

/// <summary>Resolved, immutable settings.</summary>
public sealed class SideSeatSettings
{
    /// <summary>The local SideSeat server.</summary>
    public const string DefaultEndpoint = "http://127.0.0.1:5388";

    /// <summary>The project every server starts with.</summary>
    public const string DefaultProject = "default";

    internal SideSeatSettings(
        Uri endpoint,
        string project,
        string? apiKey,
        string? serviceName,
        string? serviceVersion,
        IReadOnlyList<string> integrations,
        IReadOnlyList<string> sources,
        bool captureContent,
        bool disabled,
        bool export,
        Action<TracerProviderBuilder>? configure)
    {
        Endpoint = endpoint;
        Project = project;
        ApiKey = apiKey;
        ServiceName = serviceName;
        ServiceVersion = serviceVersion;
        Integrations = integrations;
        Sources = sources;
        CaptureContent = captureContent;
        Disabled = disabled;
        Export = export;
        Configure = configure;
    }

    /// <summary>The server or OTLP base URL.</summary>
    public Uri Endpoint { get; }

    /// <summary>The project receiving the telemetry.</summary>
    public string Project { get; }

    /// <summary>The bearer token, if any.</summary>
    public string? ApiKey { get; }

    /// <summary>The configured service name, if any.</summary>
    public string? ServiceName { get; }

    /// <summary>The configured service version, if any.</summary>
    public string? ServiceVersion { get; }

    /// <summary>Integration names, primary first.</summary>
    public IReadOnlyList<string> Integrations { get; }

    /// <summary>Additional activity sources.</summary>
    public IReadOnlyList<string> Sources { get; }

    /// <summary>Whether message content is recorded.</summary>
    public bool CaptureContent { get; }

    /// <summary>Whether the client is a no-op.</summary>
    public bool Disabled { get; }

    /// <summary>Whether spans are exported over OTLP.</summary>
    public bool Export { get; }

    internal Action<TracerProviderBuilder>? Configure { get; }

    /// <summary>
    /// The OTLP base URL. An endpoint without a path is a SideSeat server, so the project's
    /// <c>/otel/{project}</c> base is used; an endpoint with a path already is one.
    /// </summary>
    public Uri OtlpBase
    {
        get
        {
            var path = Endpoint.AbsolutePath.TrimEnd('/');
            var builder = new UriBuilder(Endpoint) { Query = string.Empty, Fragment = string.Empty };
            builder.Path = path.Length == 0 ? $"/otel/{Uri.EscapeDataString(Project)}" : path;
            return builder.Uri;
        }
    }

    /// <summary>Where the given signal is exported: <c>{OtlpBase}/v1/{signal}</c>.</summary>
    public Uri SignalEndpoint(string signal)
    {
        var builder = new UriBuilder(OtlpBase);
        builder.Path = $"{builder.Path.TrimEnd('/')}/v1/{signal}";
        return builder.Uri;
    }
}
