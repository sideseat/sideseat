using System;
using System.Collections.Generic;
using System.Diagnostics;
using OpenTelemetry.Trace;
using Xunit;

namespace SideSeat.Tests;

public sealed class SideSeatClientTests
{
    [Fact]
    public void ResolvesSideSeatAndCollectorEndpoints()
    {
        using var root = CreateClient(out _, endpoint: "http://localhost:5388");
        Assert.Equal(
            "http://localhost:5388/otel/project-a/v1/traces",
            root.TraceEndpoint.AbsoluteUri.TrimEnd('/'));

        using var project = CreateClient(out _, endpoint: "http://collector:4318/otel/custom");
        Assert.Equal(
            "http://collector:4318/otel/custom/v1/traces",
            project.TraceEndpoint.AbsoluteUri.TrimEnd('/'));

        using var complete = CreateClient(
            out _,
            endpoint: "http://collector:4318/otel/custom/v1/traces");
        Assert.Equal(
            "http://collector:4318/otel/custom/v1/traces",
            complete.TraceEndpoint.AbsoluteUri.TrimEnd('/'));
    }

    [Fact]
    public void RootTraceIsDetachedAndChildrenKeepSessionContext()
    {
        using var client = CreateClient(out var exported);
        using var ambientSource = new ActivitySource("ambient");
        using var listener = ListenTo(ambientSource.Name);
        using var ambient = ambientSource.StartActivity("ambient-root");
        Assert.NotNull(ambient);

        string traceId;
        using (var trace = client.StartTrace("agent-run", sessionId: "session-1", userId: "user-1"))
        {
            Assert.NotNull(trace.Activity);
            Assert.Null(trace.Activity!.ParentId);
            Assert.Same(trace.Activity, Activity.Current);
            Assert.Equal("session-1", trace.Activity.GetTagItem("session.id"));
            traceId = trace.Activity.TraceId.ToHexString();

            using var child = client.StartSpan("model-call", ActivityKind.Client);
            Assert.NotNull(child.Activity);
            Assert.Equal(traceId, child.Activity!.TraceId.ToHexString());
            Assert.Equal(trace.Activity.SpanId, child.Activity.ParentSpanId);
            Assert.Equal("session-1", child.Activity.GetTagItem("session.id"));
            Assert.Equal("user-1", child.Activity.GetTagItem("user.id"));
        }

        Assert.True(client.ForceFlush());
        Assert.Collection(
            exported,
            child =>
            {
                Assert.Equal("model-call", child.DisplayName);
                Assert.Equal("session-1", child.GetTagItem("session.id"));
            },
            trace =>
            {
                Assert.Equal("agent-run", trace.DisplayName);
                Assert.Equal(default, trace.ParentSpanId);
            });
        Assert.Same(ambient, Activity.Current);
    }

    [Fact]
    public void NestedContextOverridesAreRestored()
    {
        using var client = CreateClient(out var exported);

        using (client.StartTrace("run", sessionId: "outer", userId: "u1"))
        {
            using (client.StartSpan("override", sessionId: "inner", userId: "u2"))
            {
                using var nested = client.StartSpan("nested");
                Assert.Equal("inner", nested.Activity?.GetTagItem("session.id"));
                Assert.Equal("u2", nested.Activity?.GetTagItem("user.id"));
            }

            using var restored = client.StartSpan("restored");
            Assert.Equal("outer", restored.Activity?.GetTagItem("session.id"));
            Assert.Equal("u1", restored.Activity?.GetTagItem("user.id"));
        }

        Assert.True(client.ForceFlush());
        Assert.Equal(4, exported.Count);
    }

    [Fact]
    public void ExceptionsAreStructuredAndFailed()
    {
        using var client = CreateClient(out var exported);
        using (var span = client.StartTrace("failed"))
        {
            span.RecordException(new InvalidOperationException("broken"));
        }

        Assert.True(client.ForceFlush());
        var activity = Assert.Single(exported);
        Assert.Equal(ActivityStatusCode.Error, activity.Status);
        var error = Assert.Single(activity.Events);
        Assert.Equal("exception", error.Name);
        Assert.Equal(
            typeof(InvalidOperationException).FullName,
            FindTag(error.Tags, "exception.type"));
        Assert.Equal("broken", FindTag(error.Tags, "exception.message"));
    }

    [Fact]
    public void DisabledClientIsARealNoOp()
    {
        var options = new SideSeatOptions("custom")
        {
            Disabled = true,
        };
        using var client = new SideSeatClient(options);
        using var trace = client.StartTrace("ignored", sessionId: "session");
        Assert.Null(trace.Activity);
        Assert.True(client.ForceFlush());
    }

    [Fact]
    public void ConcurrentClientsDoNotDuplicateEachOthersSpans()
    {
        using var first = CreateClient(out var firstExported);
        using var second = CreateClient(out var secondExported);

        using (first.StartTrace("first"))
        {
        }
        Assert.True(first.ForceFlush());
        Assert.True(second.ForceFlush());
        Assert.Equal("first", Assert.Single(firstExported).DisplayName);
        Assert.Empty(secondExported);

        using (second.StartTrace("second"))
        {
        }
        Assert.True(first.ForceFlush());
        Assert.True(second.ForceFlush());
        Assert.Equal("second", Assert.Single(secondExported).DisplayName);
        Assert.Single(firstExported);
    }

    [Fact]
    public void PackageAndRuntimeVersionsStayAligned()
    {
        var informationalVersion = typeof(SideSeatClient).Assembly
            .GetCustomAttributes(typeof(System.Reflection.AssemblyInformationalVersionAttribute), false);
        var attribute = Assert.IsType<System.Reflection.AssemblyInformationalVersionAttribute>(
            Assert.Single(informationalVersion));
        Assert.StartsWith(SideSeatClient.Version, attribute.InformationalVersion);
        Assert.Equal("0.2.0", SideSeatClient.Version);
    }

    private static SideSeatClient CreateClient(
        out List<Activity> exported,
        string endpoint = "http://localhost:5388")
    {
        exported = new List<Activity>();
        var activities = exported;
        var options = new SideSeatOptions("custom")
        {
            Endpoint = new Uri(endpoint),
            ProjectId = "project-a",
            ExportTraces = false,
            ConfigureTracerProvider = builder => builder.AddInMemoryExporter(activities),
        };
        return new SideSeatClient(options);
    }

    private static ActivityListener ListenTo(string sourceName)
    {
        var listener = new ActivityListener
        {
            ShouldListenTo = source => source.Name == sourceName,
            Sample = (ref ActivityCreationOptions<ActivityContext> _) =>
                ActivitySamplingResult.AllDataAndRecorded,
        };
        ActivitySource.AddActivityListener(listener);
        return listener;
    }

    private static object? FindTag(
        IEnumerable<KeyValuePair<string, object?>> tags,
        string name)
    {
        foreach (var tag in tags)
        {
            if (tag.Key == name)
            {
                return tag.Value;
            }
        }
        return null;
    }
}
