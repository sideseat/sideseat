using System;
using System.Collections.Generic;
using System.Diagnostics;
using System.Diagnostics.Metrics;
using System.IO;
using System.Linq;
using System.Runtime.CompilerServices;
using System.Threading;
using System.Threading.Tasks;
using Microsoft.Extensions.AI;
using Microsoft.Extensions.Logging;
using Microsoft.Extensions.Logging.Abstractions;
using OpenTelemetry;
using OpenTelemetry.Resources;
using OpenTelemetry.Trace;
using Xunit;

namespace SideSeat.Tests;

// The client and the environment are process-wide, so these tests must not run in parallel.
[Collection("SideSeat")]
public sealed class SideSeatClientTests : IDisposable
{
    private readonly Dictionary<string, string?> _savedEnvironment = new Dictionary<string, string?>();

    public void Dispose()
    {
        SideSeatClient.Current?.Shutdown();
        foreach (var (name, value) in _savedEnvironment)
        {
            Environment.SetEnvironmentVariable(name, value);
        }
    }

    private void SetEnvironment(string name, string? value)
    {
        if (!_savedEnvironment.ContainsKey(name))
        {
            _savedEnvironment[name] = Environment.GetEnvironmentVariable(name);
        }
        Environment.SetEnvironmentVariable(name, value);
    }

    private static SideSeatOptions Recording(List<Activity> sink, params string[] integrations)
    {
        var options = new SideSeatOptions
        {
            Endpoint = "http://127.0.0.1:1",
            Export = false,
            Integrations = integrations,
            ConfigureTracerProvider = builder => builder.AddInMemoryExporter(sink),
        };
        options.Sources.Add("framework");
        return options;
    }

    private static SideSeatClient Capture(out List<Activity> exported, params string[] integrations)
    {
        exported = new List<Activity>();
        return SideSeatClient.Create(Recording(exported, integrations));
    }

    [Fact]
    public void ResolvesServerAndCollectorEndpoints()
    {
        Assert.Equal(
            "http://localhost:5388/otel/project%20a/v1/traces",
            new SideSeatOptions { Endpoint = "http://localhost:5388/", Project = "project a" }
                .Resolve().SignalEndpoint("traces").AbsoluteUri);
        Assert.Equal(
            "http://collector:4318/otel/custom/v1/metrics",
            new SideSeatOptions { Endpoint = "http://collector:4318/otel/custom/" }
                .Resolve().SignalEndpoint("metrics").AbsoluteUri);
    }

    [Theory]
    [InlineData("ftp://host")]
    [InlineData("localhost:5388")]
    public void RejectsEndpointsThatAreNotHttp(string endpoint)
    {
        Assert.Throws<SideSeatConfigurationException>(() => new SideSeatOptions { Endpoint = endpoint }.Resolve());
    }

    [Fact]
    public void ExplicitOptionsWinAndBlankValuesCountAsUnset()
    {
        SetEnvironment("SIDESEAT_ENDPOINT", " ");
        SetEnvironment("OTEL_EXPORTER_OTLP_ENDPOINT", "http://collector:4318");
        SetEnvironment("SIDESEAT_PROJECT_ID", "from-env");
        SetEnvironment("SIDESEAT_API_KEY", "env-key");
        SetEnvironment("OTEL_SERVICE_NAME", "env-service");

        var settings = new SideSeatOptions { ApiKey = "", Project = "explicit", ServiceName = " " }.Resolve();

        Assert.Equal("http://collector:4318/otel/explicit/v1/traces", settings.SignalEndpoint("traces").AbsoluteUri);
        Assert.Equal("env-key", settings.ApiKey);
        Assert.Equal("env-service", settings.ServiceName);
    }

    [Theory]
    [InlineData("1", true)]
    [InlineData("TRUE", true)]
    [InlineData("Yes", true)]
    [InlineData("0", false)]
    [InlineData("False", false)]
    [InlineData("NO", false)]
    public void FlagsAcceptEveryDocumentedSpellingInAnyCase(string raw, bool expected)
    {
        SetEnvironment("SIDESEAT_CAPTURE_CONTENT", raw);
        Assert.Equal(expected, new SideSeatOptions().Resolve().CaptureContent);
        Assert.True(new SideSeatOptions { CaptureContent = true }.Resolve().CaptureContent);
    }

    [Fact]
    public void AnInvalidFlagIsAnErrorNotADefault()
    {
        SetEnvironment("SIDESEAT_DISABLED", "maybe");
        var error = Assert.Throws<SideSeatConfigurationException>(() => new SideSeatOptions().Resolve());
        Assert.Contains("SIDESEAT_DISABLED", error.Message);
    }

    [Fact]
    public void IntegrationsComeFromTheEnvironmentWhenNotSet()
    {
        SetEnvironment("SIDESEAT_INTEGRATIONS", "semantic-kernel, openai,");
        Assert.Equal(new[] { "semantic-kernel", "openai" }, new SideSeatOptions().Resolve().Integrations);
        Assert.Empty(new SideSeatOptions { Integrations = [] }.Resolve().Integrations!);
    }

    [Fact]
    public void TheApiKeyReplacesAnAuthorizationHeaderOfAnySpellingAndKeepsTheRest()
    {
        SetEnvironment("OTEL_EXPORTER_OTLP_HEADERS", "x-team=ai%20platform,authorization=Basic old");

        Assert.Equal(
            "x-team=ai%20platform,Authorization=Bearer k",
            new SideSeatOptions { ApiKey = "k" }.Resolve().ExportHeaders());
        Assert.Equal(
            "x-team=ai%20platform,authorization=Basic old",
            new SideSeatOptions().Resolve().ExportHeaders());
    }

    [Fact]
    public void TraceIsARootEvenInsideAnotherActivity()
    {
        using var client = Capture(out _);
        using var outer = client.StartTrace("outer");
        using var inner = client.StartTrace("inner", kind: ActivityKind.Server);
        Assert.Equal(default, inner.Activity!.ParentSpanId);
        Assert.NotEqual(outer.Activity!.TraceId, inner.Activity.TraceId);
        Assert.Equal(ActivityKind.Server, inner.Activity.Kind);
    }

    [Fact]
    public void SpanIsAChildOfTheCurrentActivity()
    {
        using var client = Capture(out _);
        using var root = client.StartTrace("root");
        using var child = client.StartSpan("child", ActivityKind.Client);
        Assert.Equal(root.Activity!.SpanId, child.Activity!.ParentSpanId);
    }

    [Fact]
    public async Task SessionReachesActivitiesAFrameworkCreatesAcrossAwaits()
    {
        using var framework = new ActivitySource("framework");
        List<Activity> exported;
        using (var client = Capture(out exported))
        {
            using (client.Session("s-1", userId: "u-1"))
            {
                await Task.Yield();
                using var activity = framework.StartActivity("llm");
            }
            using var after = framework.StartActivity("after");
        }
        var llm = exported.Single(a => a.DisplayName == "llm");
        Assert.Equal("s-1", llm.GetTagItem("session.id"));
        Assert.Equal("u-1", llm.GetTagItem("user.id"));
        Assert.Null(exported.Single(a => a.DisplayName == "after").GetTagItem("session.id"));
    }

    [Fact]
    public void NestedSessionOverridesAndRestores()
    {
        List<Activity> exported;
        using (var client = Capture(out exported))
        {
            using (client.Session("outer", userId: "u"))
            {
                using (client.Session("inner"))
                using (client.StartSpan("a"))
                {
                }
                using (client.StartSpan("b"))
                {
                }
            }
        }
        Assert.Equal("inner", exported.Single(a => a.DisplayName == "a").GetTagItem("session.id"));
        Assert.Equal("u", exported.Single(a => a.DisplayName == "a").GetTagItem("user.id"));
        Assert.Equal("outer", exported.Single(a => a.DisplayName == "b").GetTagItem("session.id"));
    }

    [Fact]
    public void TraceCorrelationAppliesToDescendantsOnly()
    {
        List<Activity> exported;
        using (var client = Capture(out exported))
        {
            using (client.StartTrace("conversation", sessionId: "s-2"))
            using (client.StartSpan("step"))
            {
            }
            using (client.StartSpan("unrelated"))
            {
            }
        }
        Assert.Equal("s-2", exported.Single(a => a.DisplayName == "conversation").GetTagItem("session.id"));
        Assert.Equal("s-2", exported.Single(a => a.DisplayName == "step").GetTagItem("session.id"));
        Assert.Null(exported.Single(a => a.DisplayName == "unrelated").GetTagItem("session.id"));
    }

    [Fact]
    public void EmptySessionAndUserIdsAreRejected()
    {
        Assert.Throws<ArgumentException>(() => new SideSeatSession(""));
        Assert.Throws<ArgumentException>(() => new SideSeatSession("s", userId: ""));
        using var client = Capture(out _);
        Assert.Throws<ArgumentException>(() => client.StartTrace("t", userId: ""));
    }

    [Fact]
    public void ResourceNamesTheSdkThePrimaryIntegrationAndItsPackage()
    {
        SetEnvironment("OTEL_RESOURCE_ATTRIBUTES", "deployment.environment.name=staging,team=ai");
        var probe = new ResourceProbe();
        var options = new SideSeatOptions
        {
            Export = false,
            Integrations = ["extensions-ai"],
            ConfigureTracerProvider = builder => builder.AddProcessor(probe),
        };
        options.ResourceAttributes["team"] = "platform";
        using var client = SideSeatClient.Create(options);

        var resource = probe.Attributes();
        Assert.Equal(new[] { "extensions-ai" }, client.Integrations);
        Assert.Equal("Microsoft.Extensions.AI", resource["service.name"]);
        Assert.Equal(PackageVersion(typeof(ChatClientBuilder)), resource["service.version"]);
        Assert.Equal("sideseat", resource["telemetry.sdk.name"]);
        Assert.Equal("dotnet", resource["telemetry.sdk.language"]);
        Assert.Equal(SideSeatClient.Version, resource["telemetry.sdk.version"]);
        Assert.Equal("extensions-ai", resource["sideseat.framework"]);
        Assert.Equal(new[] { "extensions-ai" }, (string[])resource["sideseat.integrations"]);
        Assert.Equal("staging", resource["deployment.environment.name"]);
        Assert.Equal("platform", resource["team"]);
    }

    [Fact]
    public void WithoutIntegrationsTheServiceIsTheAppAndTheSdkVersion()
    {
        var probe = new ResourceProbe();
        using var client = SideSeatClient.Create(new SideSeatOptions
        {
            Export = false,
            Integrations = [],
            ConfigureTracerProvider = builder => builder.AddProcessor(probe),
        });

        var resource = probe.Attributes();
        Assert.Equal("sideseat-app", resource["service.name"]);
        Assert.Equal(SideSeatClient.Version, resource["service.version"]);
        Assert.False(resource.ContainsKey("sideseat.framework"));
    }

    [Fact]
    public void DetectsTheInstalledFrameworkWhenNoneIsRequested()
    {
        using var client = SideSeatClient.Create(new SideSeatOptions { Export = false });
        Assert.Equal(new[] { "extensions-ai" }, client.Integrations);
    }

    [Fact]
    public void ARequestedIntegrationMustBeKnownAndInstalled()
    {
        var unknown = Assert.Throws<SideSeatIntegrationException>(
            () => SideSeatClient.Create(new SideSeatOptions { Export = false, Integrations = ["semantic-kernal"] }));
        Assert.Contains("semantic-kernel", unknown.Message);
        var missing = Assert.Throws<SideSeatIntegrationException>(
            () => SideSeatClient.Create(new SideSeatOptions { Export = false, Integrations = ["semantic-kernel"] }));
        Assert.Contains("dotnet add package Microsoft.SemanticKernel", missing.Message);
        Assert.Null(SideSeatClient.Current);
    }

    [Fact]
    public void IntegrationSwitchesAreRestoredAtShutdown()
    {
        const string name = "OpenAI.Experimental.EnableOpenTelemetry";
        var client = SideSeatClient.Create(new SideSeatOptions { Export = false, Integrations = ["openai"] });
        Assert.Equal(new[] { "openai" }, client.Integrations);
        Assert.True(AppContext.TryGetSwitch(name, out var enabled) && enabled);
        client.Shutdown();
        Assert.True(AppContext.TryGetSwitch(name, out var after) && !after);
    }

    [Fact]
    public async Task ExtensionsAiCallsCarryTheSessionAndTheirContent()
    {
        SetEnvironment("OTEL_INSTRUMENTATION_GENAI_CAPTURE_MESSAGE_CONTENT", null);
        List<Activity> exported;
        using (var client = Capture(out exported, "extensions-ai"))
        {
            using IChatClient chat = new EchoChatClient().AsBuilder().UseOpenTelemetry().Build();
            using (client.Session("conversation-42", userId: "user-7"))
            {
                await chat.GetResponseAsync("Plan a weekend in Lisbon.", cancellationToken: TestContext.Current.CancellationToken);
            }
        }
        var call = exported.Single(a => a.Source.Name.StartsWith("Experimental.Microsoft.Extensions.AI", StringComparison.Ordinal));
        Assert.Equal("conversation-42", call.GetTagItem("session.id"));
        Assert.Equal("user-7", call.GetTagItem("user.id"));
        Assert.Contains("Plan a weekend in Lisbon.", (string)call.GetTagItem("gen_ai.input.messages")!);
    }

    [Fact]
    public void AHostedPipelineGetsCorrelationWithoutAClient()
    {
        var exported = new List<Activity>();
        using var framework = new ActivitySource("framework");
        var options = new SideSeatOptions { Export = false, Integrations = [] };
        options.Sources.Add("framework");
        using (var provider = Sdk.CreateTracerProviderBuilder()
            .AddSideSeat(options)
            .AddInMemoryExporter(exported)
            .Build())
        {
            using (new SideSeatSession("hosted", userId: "u"))
            using (framework.StartActivity("llm"))
            {
            }
        }
        Assert.Equal("hosted", exported.Single().GetTagItem("session.id"));
        Assert.Null(SideSeatClient.Current);
    }

    [Fact]
    public void CreateReturnsTheRunningClientForTheSameOptionsOnly()
    {
        using var client = SideSeatClient.Create(new SideSeatOptions { Export = false, Integrations = [] });

        Assert.Same(client, SideSeatClient.Create(new SideSeatOptions { Export = false, Integrations = [] }));
        Assert.Throws<SideSeatConfigurationException>(
            () => SideSeatClient.Create(new SideSeatOptions { Export = false, Integrations = [], Project = "other" }));
        Assert.Throws<SideSeatConfigurationException>(
            () => SideSeatClient.Create(Recording(new List<Activity>())));
    }

    [Fact]
    public void DisabledRecordsNothing()
    {
        SetEnvironment("SIDESEAT_DISABLED", "true");
        using var client = SideSeatClient.Create();
        using var span = client.StartTrace("ignored", sessionId: "s");
        Assert.Null(span.Activity);
        Assert.Empty(client.Integrations);
        Assert.Same(NullLoggerFactory.Instance, client.LoggerFactory);
        Assert.True(client.Flush());
        Assert.True(client.Shutdown());
    }

    [Fact]
    public void DebugWritesTheResolvedConfigurationToStandardError()
    {
        var original = Console.Error;
        var captured = new StringWriter();
        Console.SetError(captured);
        try
        {
            using var client = SideSeatClient.Create(new SideSeatOptions { Export = false, Integrations = [], Debug = true });
        }
        finally
        {
            Console.SetError(original);
        }
        Assert.Contains("[sideseat] exporting to http://127.0.0.1:5388/otel/default", captured.ToString());
    }

    [Fact]
    public void ShutdownIsIdempotentAndAllowsANewClient()
    {
        var client = Capture(out _);
        Assert.True(client.Shutdown());
        Assert.True(client.Shutdown());
        Assert.Null(SideSeatClient.Current);
        using var next = Capture(out _);
        Assert.Same(next, SideSeatClient.Current);
    }

    [Fact]
    public void ExportsEverySignalOverOtlpHttpWithMergedHeaders()
    {
        SetEnvironment("OTEL_EXPORTER_OTLP_HEADERS", "x-team=ai");
        using var collector = new OtlpCollector();
        using var meter = new Meter("framework");
        var options = new SideSeatOptions
        {
            Endpoint = collector.Endpoint,
            Project = "team a",
            ApiKey = "k-1",
            Integrations = [],
        };
        options.Sources.Add("framework");
        using var client = SideSeatClient.Create(options);
        using (client.StartTrace("agent-run", sessionId: "s-1"))
        {
        }
        meter.CreateCounter<long>("app.requests").Add(1);
        client.LoggerFactory.CreateLogger("app").LogInformation("planned the trip");
        Assert.True(client.Flush());

        // Taken as Flush returns: a request still on its way would mean Flush returned too early.
        var requests = collector.Requests;
        Assert.Equal(
            new[] { "/otel/team%20a/v1/logs", "/otel/team%20a/v1/metrics", "/otel/team%20a/v1/traces" },
            requests.Select(r => r.Path).Distinct().OrderBy(p => p, StringComparer.Ordinal));
        Assert.All(requests, r => Assert.Equal("Bearer k-1", r.Authorization));
        Assert.All(requests, r => Assert.Equal("ai", r.Team));
        Assert.Contains(requests, r => r.Path.EndsWith("/traces", StringComparison.Ordinal) && r.Body.Contains("s-1"));
        Assert.Contains(requests, r => r.Path.EndsWith("/logs", StringComparison.Ordinal) && r.Body.Contains("planned the trip"));
        Assert.Contains(requests, r => r.Path.EndsWith("/metrics", StringComparison.Ordinal) && r.Body.Contains("app.requests"));
    }

    private static string PackageVersion(Type type) =>
        type.Assembly
            .GetCustomAttributes(typeof(System.Reflection.AssemblyInformationalVersionAttribute), false)
            .Cast<System.Reflection.AssemblyInformationalVersionAttribute>()
            .Single().InformationalVersion.Split('+')[0];

    /// <summary>Reads the resource of the provider it is added to.</summary>
    private sealed class ResourceProbe : BaseProcessor<Activity>
    {
        public Dictionary<string, object> Attributes() =>
            ParentProvider!.GetResource().Attributes.ToDictionary(kv => kv.Key, kv => kv.Value);
    }

    /// <summary>A chat client that answers with the prompt, so a test needs no model.</summary>
    private sealed class EchoChatClient : IChatClient
    {
        public Task<ChatResponse> GetResponseAsync(
            IEnumerable<ChatMessage> messages,
            ChatOptions? options = null,
            CancellationToken cancellationToken = default) =>
            Task.FromResult(new ChatResponse(new ChatMessage(ChatRole.Assistant, messages.Last().Text)));

        public async IAsyncEnumerable<ChatResponseUpdate> GetStreamingResponseAsync(
            IEnumerable<ChatMessage> messages,
            ChatOptions? options = null,
            [EnumeratorCancellation] CancellationToken cancellationToken = default)
        {
            await Task.Yield();
            yield return new ChatResponseUpdate(ChatRole.Assistant, messages.Last().Text);
        }

        public object? GetService(Type serviceType, object? serviceKey = null) => null;

        public void Dispose()
        {
        }
    }
}

[CollectionDefinition("SideSeat", DisableParallelization = true)]
public sealed class SideSeatCollection
{
}
