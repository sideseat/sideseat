using System;
using System.Collections.Generic;
using System.Diagnostics;
using System.Linq;
using System.Threading.Tasks;
using OpenTelemetry.Trace;
using Xunit;

namespace SideSeat.Tests;

// The client is process-wide, so these tests must not run in parallel with each other.
[Collection("SideSeat")]
public sealed class SideSeatClientTests
{
    private static SideSeatClient Capture(out List<Activity> exported, params string[] integrations)
    {
        var sink = new List<Activity>();
        exported = sink;
        var options = new SideSeatOptions
        {
            Endpoint = "http://127.0.0.1:1",
            Export = false,
            ConfigureTracerProvider = builder => builder.AddInMemoryExporter(sink),
        };
        foreach (var integration in integrations)
        {
            options.Integrations.Add(integration);
        }
        options.Sources.Add("framework");
        return SideSeatClient.Create(options);
    }

    [Fact]
    public void ResolvesServerAndCollectorEndpoints()
    {
        Assert.Equal(
            "http://localhost:5388/otel/project-a/v1/traces",
            new SideSeatOptions { Endpoint = "http://localhost:5388/", Project = "project-a" }
                .Resolve().SignalEndpoint("traces").AbsoluteUri);
        Assert.Equal(
            "http://collector:4318/otel/custom/v1/traces",
            new SideSeatOptions { Endpoint = "http://collector:4318/otel/custom" }
                .Resolve().SignalEndpoint("traces").AbsoluteUri);
    }

    [Theory]
    [InlineData("ftp://host")]
    [InlineData("localhost:5388")]
    public void RejectsEndpointsThatAreNotHttp(string endpoint)
    {
        Assert.Throws<SideSeatConfigurationException>(() => new SideSeatOptions { Endpoint = endpoint }.Resolve());
    }

    [Fact]
    public void TraceIsARootEvenInsideAnotherActivity()
    {
        using (var client = Capture(out var exported))
        {
            using var outer = client.StartTrace("outer");
            using var inner = client.StartTrace("inner");
            Assert.Equal(default, inner.Activity!.ParentSpanId);
            Assert.NotEqual(outer.Activity!.TraceId, inner.Activity.TraceId);
        }
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
    public void ResourceNamesTheSdkAndThePrimaryIntegration()
    {
        using var client = Capture(out _, "extensions-ai");
        Assert.Equal(new[] { "extensions-ai" }, client.Integrations);
    }

    [Fact]
    public void ASecondLiveClientIsAnError()
    {
        using var client = Capture(out _);
        Assert.Throws<SideSeatConfigurationException>(() => SideSeatClient.Create(new SideSeatOptions { Export = false }));
    }

    [Fact]
    public void UnknownIntegrationsListTheKnownOnes()
    {
        var error = Assert.Throws<SideSeatConfigurationException>(() => SideSeatIntegrations.Get("semantic-kernal"));
        Assert.Contains("semantic-kernel", error.Message);
    }

    [Fact]
    public void DisabledRecordsNothing()
    {
        using var client = SideSeatClient.Create(new SideSeatOptions { Disabled = true });
        using var span = client.StartTrace("ignored", sessionId: "s");
        Assert.Null(span.Activity);
        Assert.True(client.Flush());
    }

    [Fact]
    public void ShutdownIsIdempotentAndAllowsANewClient()
    {
        var client = Capture(out _);
        Assert.True(client.Shutdown());
        Assert.True(client.Shutdown());
        using var next = Capture(out _);
        Assert.Same(next, SideSeatClient.Current);
    }
}
