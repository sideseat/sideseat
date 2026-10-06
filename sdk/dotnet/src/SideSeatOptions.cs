using System;
using System.Collections.Generic;
using System.Globalization;
using System.Linq;
using OpenTelemetry.Trace;

namespace SideSeat;

/// <summary>
/// Configuration for <see cref="SideSeatClient"/>. Every property falls back to its environment
/// variable, then to a default; an empty or blank value counts as unset.
/// </summary>
public sealed class SideSeatOptions
{
    /// <summary>SideSeat server URL, or an OTLP base URL that already has a path. <c>SIDESEAT_ENDPOINT</c>, then <c>OTEL_EXPORTER_OTLP_ENDPOINT</c>.</summary>
    public string? Endpoint { get; set; }

    /// <summary>Project that receives the telemetry. <c>SIDESEAT_PROJECT_ID</c>.</summary>
    public string? Project { get; set; }

    /// <summary>Sent as a bearer token. <c>SIDESEAT_API_KEY</c>.</summary>
    public string? ApiKey { get; set; }

    /// <summary><c>service.name</c>. <c>OTEL_SERVICE_NAME</c>; defaults to the primary integration's package.</summary>
    public string? ServiceName { get; set; }

    /// <summary><c>service.version</c>. <c>OTEL_SERVICE_VERSION</c>; defaults to that package's version.</summary>
    public string? ServiceVersion { get; set; }

    /// <summary>
    /// Integration names; the first is the primary one. <c>SIDESEAT_INTEGRATIONS</c> when null, and
    /// detected from the installed packages when that is unset too. An empty list means none. Known
    /// names are listed in <see cref="SideSeatIntegrations.Names"/>.
    /// </summary>
    public IReadOnlyList<string>? Integrations { get; set; }

    /// <summary>Additional activity sources and meters to export, such as the application's own.</summary>
    public IList<string> Sources { get; } = new List<string>();

    /// <summary>Extra attributes on the resource of every signal, over <c>OTEL_RESOURCE_ATTRIBUTES</c>.</summary>
    public IDictionary<string, object> ResourceAttributes { get; } = new Dictionary<string, object>();

    /// <summary>Record prompts, responses, and tool payloads. <c>SIDESEAT_CAPTURE_CONTENT</c>; on by default.</summary>
    public bool? CaptureContent { get; set; }

    /// <summary>Configure nothing; every call becomes a no-op. <c>SIDESEAT_DISABLED</c>.</summary>
    public bool? Disabled { get; set; }

    /// <summary>Write the resolved configuration to standard error. <c>SIDESEAT_DEBUG</c>.</summary>
    public bool? Debug { get; set; }

    /// <summary>Send telemetry over OTLP. Off is useful with <see cref="ConfigureTracerProvider"/> in tests.</summary>
    public bool Export { get; set; } = true;

    /// <summary>Export metrics from the integrations' meters and <see cref="Sources"/>. On by default.</summary>
    public bool Metrics { get; set; } = true;

    /// <summary>Build <see cref="SideSeatClient.LoggerFactory"/>, whose log records are exported. On by default.</summary>
    public bool Logs { get; set; } = true;

    /// <summary>Add processors or exporters before the tracer provider is built.</summary>
    public Action<TracerProviderBuilder>? ConfigureTracerProvider { get; set; }

    /// <summary>Resolves these options against the environment.</summary>
    internal SideSeatSettings Resolve()
    {
        var endpoint = ParseEndpoint(
            Text(Endpoint, "SIDESEAT_ENDPOINT") ?? Text(null, "OTEL_EXPORTER_OTLP_ENDPOINT") ?? SideSeatSettings.DefaultEndpoint);
        var integrations = Integrations?.ToList() ?? Text(null, "SIDESEAT_INTEGRATIONS")?
            .Split(',')
            .Select(name => name.Trim())
            .Where(name => name.Length > 0)
            .ToList();
        var content = CaptureContent ?? Flag("SIDESEAT_CAPTURE_CONTENT", true);
        return new SideSeatSettings(
            endpoint,
            Text(Project, "SIDESEAT_PROJECT_ID") ?? SideSeatSettings.DefaultProject,
            Text(ApiKey, "SIDESEAT_API_KEY"),
            Text(ServiceName, "OTEL_SERVICE_NAME"),
            Text(ServiceVersion, "OTEL_SERVICE_VERSION"),
            integrations,
            Sources.ToList(),
            new Dictionary<string, object>(ResourceAttributes),
            content,
            CaptureContent.HasValue || !content,
            Disabled ?? Flag("SIDESEAT_DISABLED", false),
            Debug ?? Flag("SIDESEAT_DEBUG", false),
            Export,
            Metrics,
            Logs,
            ConfigureTracerProvider);
    }

    private static string? Text(string? explicitValue, string name)
    {
        if (!string.IsNullOrWhiteSpace(explicitValue))
        {
            return explicitValue!.Trim();
        }
        var value = Environment.GetEnvironmentVariable(name);
        return string.IsNullOrWhiteSpace(value) ? null : value.Trim();
    }

    private static bool Flag(string name, bool fallback)
    {
        var raw = Text(null, name);
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
        IReadOnlyList<string>? integrations,
        IReadOnlyList<string> sources,
        IReadOnlyDictionary<string, object> resourceAttributes,
        bool captureContent,
        bool captureContentOverrides,
        bool disabled,
        bool debug,
        bool export,
        bool metrics,
        bool logs,
        Action<TracerProviderBuilder>? configure)
    {
        Endpoint = endpoint;
        Project = project;
        ApiKey = apiKey;
        ServiceName = serviceName;
        ServiceVersion = serviceVersion;
        Integrations = integrations;
        Sources = sources;
        ResourceAttributes = resourceAttributes;
        CaptureContent = captureContent;
        CaptureContentOverrides = captureContentOverrides;
        Disabled = disabled;
        Debug = debug;
        Export = export;
        Metrics = metrics;
        Logs = logs;
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

    /// <summary>Requested integration names, primary first; null when they are detected.</summary>
    public IReadOnlyList<string>? Integrations { get; }

    /// <summary>Additional activity sources and meters.</summary>
    public IReadOnlyList<string> Sources { get; }

    /// <summary>Extra resource attributes.</summary>
    public IReadOnlyDictionary<string, object> ResourceAttributes { get; }

    /// <summary>Whether message content is recorded.</summary>
    public bool CaptureContent { get; }

    /// <summary>
    /// Whether <see cref="CaptureContent"/> overrides the instrumentations' own content switch: when
    /// it was an option, or when it is off. <c>SIDESEAT_CAPTURE_CONTENT=true</c> alone does not turn
    /// on content an application switched off in another variable.
    /// </summary>
    public bool CaptureContentOverrides { get; }

    /// <summary>Whether the client is a no-op.</summary>
    public bool Disabled { get; }

    /// <summary>Whether the resolved configuration is written to standard error.</summary>
    public bool Debug { get; }

    /// <summary>Whether telemetry is exported over OTLP.</summary>
    public bool Export { get; }

    /// <summary>Whether metrics are exported.</summary>
    public bool Metrics { get; }

    /// <summary>Whether log records are exported.</summary>
    public bool Logs { get; }

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

    /// <summary>
    /// OTLP request headers in the exporter's <c>key=value,...</c> form: <c>OTEL_EXPORTER_OTLP_HEADERS</c>
    /// plus the API key, which replaces an <c>Authorization</c> header of any spelling. Setting
    /// the exporter's headers replaces the ones it would read from the environment, so they are
    /// merged here.
    /// </summary>
    internal string ExportHeaders()
    {
        var pairs = (Environment.GetEnvironmentVariable("OTEL_EXPORTER_OTLP_HEADERS") ?? string.Empty)
            .Split(',')
            .Select(pair => pair.Trim())
            .Where(pair => pair.IndexOf('=') > 0)
            .ToList();
        if (!string.IsNullOrWhiteSpace(ApiKey))
        {
            pairs.RemoveAll(pair => string.Equals(
                Uri.UnescapeDataString(pair.Substring(0, pair.IndexOf('=')).Trim()),
                "authorization",
                StringComparison.OrdinalIgnoreCase));
            pairs.Add($"Authorization=Bearer {ApiKey}");
        }
        return string.Join(",", pairs);
    }

    /// <summary>What a second <see cref="SideSeatClient.Create"/> must match to return the same client.</summary>
    internal string Identity() => string.Join(
        "\u001f",
        new object?[]
        {
            Endpoint, Project, ApiKey, ServiceName, ServiceVersion,
            Integrations == null ? "<detect>" : string.Join(",", Integrations),
            string.Join(",", Sources),
            string.Join(",", ResourceAttributes.OrderBy(kv => kv.Key, StringComparer.Ordinal)
                .Select(kv => $"{kv.Key}={Convert.ToString(kv.Value, CultureInfo.InvariantCulture)}")),
            CaptureContent, CaptureContentOverrides, Disabled, Export, Metrics, Logs,
        });
}
