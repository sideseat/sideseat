using System;
using System.Collections.Generic;
using System.Linq;
using System.Reflection;

namespace SideSeat;

/// <summary>
/// One framework or library whose telemetry SideSeat switches on: the package that identifies it, the
/// activity sources and meters it emits through, and the switches that make it emit GenAI content.
/// </summary>
internal sealed class SideSeatIntegration
{
    internal SideSeatIntegration(
        string name,
        string package,
        string assembly,
        bool detectable,
        IReadOnlyList<string> sources,
        IReadOnlyList<KeyValuePair<string, Func<SideSeatSettings, bool>>>? switches = null)
    {
        Name = name;
        Package = package;
        Assembly = assembly;
        Detectable = detectable;
        Sources = sources;
        Switches = switches ?? Array.Empty<KeyValuePair<string, Func<SideSeatSettings, bool>>>();
    }

    /// <summary>Stable identifier, recorded as <c>sideseat.framework</c> when it is the primary one.</summary>
    internal string Name { get; }

    /// <summary>The NuGet package, reported as <c>service.name</c> when it is the primary integration.</summary>
    internal string Package { get; }

    /// <summary>An assembly the package ships, probed to tell whether the package is installed.</summary>
    internal string Assembly { get; }

    /// <summary>
    /// Whether auto-detection may choose it. Provider clients are installed transitively by
    /// frameworks, so their presence says nothing about what the application uses.
    /// </summary>
    internal bool Detectable { get; }

    /// <summary>Activity source and meter names, wildcards allowed.</summary>
    internal IReadOnlyList<string> Sources { get; }

    /// <summary><see cref="AppContext"/> switches the library reads, and the value each needs.</summary>
    internal IReadOnlyList<KeyValuePair<string, Func<SideSeatSettings, bool>>> Switches { get; }

    /// <summary>The installed package version, or null when the package is not installed.</summary>
    internal string? InstalledVersion()
    {
        try
        {
            var assembly = System.Reflection.Assembly.Load(new AssemblyName(Assembly));
            var informational = assembly
                .GetCustomAttribute<AssemblyInformationalVersionAttribute>()?.InformationalVersion;
            // Informational versions carry the source revision after '+'; the package version does not.
            return informational?.Split('+')[0] ?? assembly.GetName().Version?.ToString() ?? string.Empty;
        }
        catch (Exception error) when (error is System.IO.FileNotFoundException
            or System.IO.FileLoadException
            or BadImageFormatException)
        {
            return null;
        }
    }

    /// <summary>Sets this integration's switches and returns what restores the previous values.</summary>
    internal Action Enable(SideSeatSettings settings)
    {
        var restore = new List<Action>();
        foreach (var entry in Switches)
        {
            var name = entry.Key;
            var hadValue = AppContext.TryGetSwitch(name, out var previous);
            AppContext.SetSwitch(name, entry.Value(settings));
            // AppContext cannot forget a switch; an unset switch reads as false, so restore to that.
            restore.Add(() => AppContext.SetSwitch(name, hadValue && previous));
        }
        return () => restore.ForEach(action => action());
    }
}

/// <summary>The built-in integrations.</summary>
public static class SideSeatIntegrations
{
    private const string GenAiCaptureContent = "OTEL_INSTRUMENTATION_GENAI_CAPTURE_MESSAGE_CONTENT";

    // In auto-detection priority: a framework that depends on another is listed before it, so an
    // application built on Agent Framework is labelled by it rather than by Microsoft.Extensions.AI.
    private static readonly SideSeatIntegration[] Registry =
    {
        new SideSeatIntegration(
            "agent-framework",
            "Microsoft.Agents.AI",
            "Microsoft.Agents.AI",
            detectable: true,
            new[] { "Experimental.Microsoft.Agents.AI*", "Experimental.Microsoft.Extensions.AI*" }),
        new SideSeatIntegration(
            "semantic-kernel",
            "Microsoft.SemanticKernel",
            "Microsoft.SemanticKernel.Abstractions",
            detectable: true,
            new[] { "Microsoft.SemanticKernel*" },
            new[]
            {
                Switch("Microsoft.SemanticKernel.Experimental.GenAI.EnableOTelDiagnostics", _ => true),
                Switch(
                    "Microsoft.SemanticKernel.Experimental.GenAI.EnableOTelDiagnosticsSensitive",
                    settings => settings.CaptureContent),
            }),
        // Microsoft.Extensions.AI emits through this source once a chat client is wrapped with
        // UseOpenTelemetry(); content follows the standard GenAI capture switch.
        new SideSeatIntegration(
            "extensions-ai",
            "Microsoft.Extensions.AI",
            "Microsoft.Extensions.AI",
            detectable: true,
            new[] { "Experimental.Microsoft.Extensions.AI*" }),
        new SideSeatIntegration(
            "openai",
            "OpenAI",
            "OpenAI",
            detectable: false,
            new[] { "OpenAI.*" },
            new[] { Switch("OpenAI.Experimental.EnableOpenTelemetry", _ => true) }),
    };

    /// <summary>Every built-in integration name, in auto-detection priority order.</summary>
    public static IReadOnlyList<string> Names => Registry.Select(integration => integration.Name).ToList();

    internal static SideSeatIntegration Get(string name)
    {
        var found = Registry.FirstOrDefault(integration => integration.Name == name);
        if (found == null)
        {
            throw new SideSeatIntegrationException(
                $"Unknown integration '{name}'; known integrations: {string.Join(", ", Names)}.");
        }
        return found;
    }

    /// <summary>
    /// The integrations for <paramref name="settings"/>, with their installed versions. Requested
    /// integrations must be installed; without a request, the first installed detectable one is used.
    /// Only one is detected: frameworks bundle each other, so a second package proves nothing.
    /// </summary>
    internal static IReadOnlyList<(SideSeatIntegration Integration, string Version)> Resolve(SideSeatSettings settings)
    {
        if (settings.Integrations == null)
        {
            foreach (var candidate in Registry.Where(integration => integration.Detectable))
            {
                var detected = candidate.InstalledVersion();
                if (detected != null)
                {
                    return new[] { (candidate, detected) };
                }
            }
            return Array.Empty<(SideSeatIntegration, string)>();
        }
        var resolved = new List<(SideSeatIntegration, string)>();
        foreach (var name in settings.Integrations.Distinct())
        {
            var integration = Get(name);
            var version = integration.InstalledVersion() ?? throw new SideSeatIntegrationException(
                $"Integration '{name}' needs the {integration.Package} package. Install it with: " +
                $"dotnet add package {integration.Package}");
            resolved.Add((integration, version));
        }
        return resolved;
    }

    internal static void EnableContentCapture(SideSeatSettings settings)
    {
        if (settings.CaptureContent && string.IsNullOrEmpty(Environment.GetEnvironmentVariable(GenAiCaptureContent)))
        {
            // Read lazily by the instrumented libraries, after Create returns, so it cannot be scoped.
            Environment.SetEnvironmentVariable(GenAiCaptureContent, "true");
        }
    }

    private static KeyValuePair<string, Func<SideSeatSettings, bool>> Switch(
        string name,
        Func<SideSeatSettings, bool> value) =>
        new KeyValuePair<string, Func<SideSeatSettings, bool>>(name, value);
}
