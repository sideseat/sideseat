using System;
using System.Collections.Generic;
using System.Linq;

namespace SideSeat;

/// <summary>
/// One framework or library whose telemetry SideSeat switches on: the activity sources it emits
/// through, and the switches that make it emit GenAI content.
/// </summary>
public sealed class SideSeatIntegration
{
    /// <summary>Create an integration.</summary>
    public SideSeatIntegration(string name, IReadOnlyList<string> sources, Action<SideSeatSettings>? enable = null)
    {
        Name = name;
        Sources = sources;
        Enable = enable;
    }

    /// <summary>Stable identifier, recorded as <c>sideseat.framework</c> when it is the primary one.</summary>
    public string Name { get; }

    /// <summary>Activity source names, wildcards allowed.</summary>
    public IReadOnlyList<string> Sources { get; }

    internal Action<SideSeatSettings>? Enable { get; }
}

/// <summary>The built-in integrations.</summary>
public static class SideSeatIntegrations
{
    private const string GenAiCaptureContent = "OTEL_INSTRUMENTATION_GENAI_CAPTURE_MESSAGE_CONTENT";

    private static readonly SideSeatIntegration[] Registry =
    {
        // Microsoft.Extensions.AI emits through this source once a chat client is wrapped with
        // UseOpenTelemetry(); content follows the standard GenAI capture switch.
        new SideSeatIntegration("extensions-ai", new[] { "Experimental.Microsoft.Extensions.AI*" }),
        new SideSeatIntegration("agent-framework", new[] { "Experimental.Microsoft.Agents.AI*", "Experimental.Microsoft.Extensions.AI*" }),
        new SideSeatIntegration(
            "semantic-kernel",
            new[] { "Microsoft.SemanticKernel*" },
            settings =>
            {
                AppContext.SetSwitch("Microsoft.SemanticKernel.Experimental.GenAI.EnableOTelDiagnostics", true);
                AppContext.SetSwitch(
                    "Microsoft.SemanticKernel.Experimental.GenAI.EnableOTelDiagnosticsSensitive",
                    settings.CaptureContent);
            }),
        new SideSeatIntegration(
            "openai",
            new[] { "OpenAI.*" },
            _ => AppContext.SetSwitch("OpenAI.Experimental.EnableOpenTelemetry", true)),
    };

    /// <summary>Every built-in integration name.</summary>
    public static IReadOnlyList<string> Names => Registry.Select(integration => integration.Name).ToList();

    /// <summary>The integration registered under <paramref name="name"/>.</summary>
    public static SideSeatIntegration Get(string name)
    {
        var found = Registry.FirstOrDefault(integration => integration.Name == name);
        if (found == null)
        {
            throw new SideSeatConfigurationException(
                $"Unknown integration '{name}'; known integrations: {string.Join(", ", Names)}.");
        }
        return found;
    }

    internal static void EnableContentCapture(SideSeatSettings settings)
    {
        if (settings.CaptureContent && string.IsNullOrEmpty(Environment.GetEnvironmentVariable(GenAiCaptureContent)))
        {
            // Read lazily by the instrumented libraries, after Create returns, so it cannot be scoped.
            Environment.SetEnvironmentVariable(GenAiCaptureContent, "true");
        }
    }
}
