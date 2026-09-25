using System.Diagnostics;
using OpenTelemetry;
using OpenTelemetry.Exporter;
using OpenTelemetry.Resources;
using OpenTelemetry.Trace;
using SideSeat;

const string SessionId = "sdk-conformance-session";
const string UserId = "sdk-conformance-user";
const string InputMessages =
    """[{"role":"user","parts":[{"type":"text","content":"What is the weather in London?"}]}]""";
const string FirstOutput =
    """[{"role":"assistant","parts":[{"type":"text","content":"I will check the weather."}],"finish_reason":"tool_calls"}]""";
const string FinalOutput =
    """[{"role":"assistant","parts":[{"type":"text","content":"It is 18°C and sunny in London."}],"finish_reason":"stop"}]""";

var mode = args.SingleOrDefault()?.Trim().ToLowerInvariant();
if (mode is not ("sdk" or "otel"))
{
    Console.Error.WriteLine("usage: SideSeat.Conformance sdk|otel");
    return 2;
}

if (mode == "sdk")
{
    RunWithSideSeat();
}
else
{
    RunWithOpenTelemetry();
}

return 0;

void RunWithSideSeat()
{
    using var client = new SideSeatClient(new SideSeatOptions("dotnet-conformance")
    {
        ServiceName = "dotnet-conformance",
    });

    using (client.StartTrace("canonical-agent-run", SessionId, UserId))
    {
        using (var first = client.StartSpan("chat canonical-model", ActivityKind.Client))
        {
            AddFirstModelCall((name, value) => first.SetAttribute(name, value));
        }

        using (var tool = client.StartSpan("execute_tool get_weather"))
        {
            AddToolCall((name, value) => tool.SetAttribute(name, value));
        }

        using (var final = client.StartSpan("chat canonical-model", ActivityKind.Client))
        {
            AddFinalModelCall((name, value) => final.SetAttribute(name, value));
        }
    }

    if (!client.ForceFlush())
    {
        throw new InvalidOperationException("SideSeat SDK did not flush its spans");
    }
}

void RunWithOpenTelemetry()
{
    var endpoint = BuildTraceEndpoint();
    using var source = new ActivitySource("SideSeat.Conformance.Raw", SideSeatClient.Version);
    var resource = ResourceBuilder.CreateDefault()
        .AddService("dotnet-conformance")
        .AddAttributes(
        [
            new KeyValuePair<string, object>("sideseat.framework", "dotnet-conformance"),
        ]);
    using var provider = Sdk.CreateTracerProviderBuilder()
        .SetResourceBuilder(resource)
        .SetSampler(new AlwaysOnSampler())
        .AddSource(source.Name)
        .AddOtlpExporter(exporter =>
        {
            exporter.Endpoint = endpoint;
            exporter.Protocol = OtlpExportProtocol.HttpProtobuf;
        })
        .Build();

    using (var root = source.StartActivity(
        "canonical-agent-run",
        ActivityKind.Internal,
        default(ActivityContext),
        tags:
        [
            new KeyValuePair<string, object?>("session.id", SessionId),
            new KeyValuePair<string, object?>("user.id", UserId),
        ]))
    {
        if (root == null)
        {
            throw new InvalidOperationException("raw OpenTelemetry root span was not sampled");
        }

        using (var first = source.StartActivity("chat canonical-model", ActivityKind.Client))
        {
            AddCorrelation(first);
            AddFirstModelCall((name, value) => Set(first, name, value));
        }

        using (var tool = source.StartActivity("execute_tool get_weather"))
        {
            AddCorrelation(tool);
            AddToolCall((name, value) => Set(tool, name, value));
        }

        using (var final = source.StartActivity("chat canonical-model", ActivityKind.Client))
        {
            AddCorrelation(final);
            AddFinalModelCall((name, value) => Set(final, name, value));
        }
    }

    if (!provider.ForceFlush())
    {
        throw new InvalidOperationException("raw OpenTelemetry provider did not flush its spans");
    }
}

void AddFirstModelCall(Action<string, object?> set)
{
    set("gen_ai.operation.name", "chat");
    set("gen_ai.provider.name", "conformance");
    set("gen_ai.request.model", "canonical-model");
    set("gen_ai.input.messages", InputMessages);
    set("gen_ai.output.messages", FirstOutput);
    set("gen_ai.usage.input_tokens", 12L);
    set("gen_ai.usage.output_tokens", 6L);
}

void AddToolCall(Action<string, object?> set)
{
    set("gen_ai.operation.name", "execute_tool");
    set("gen_ai.tool.name", "get_weather");
    set("gen_ai.tool.call.id", "call-weather-1");
    set("gen_ai.tool.type", "function");
    set("gen_ai.tool.call.arguments", """{"city":"London"}""");
    set("gen_ai.tool.call.result", """{"temperature_c":18,"condition":"sunny"}""");
}

void AddFinalModelCall(Action<string, object?> set)
{
    set("gen_ai.operation.name", "chat");
    set("gen_ai.provider.name", "conformance");
    set("gen_ai.request.model", "canonical-model");
    set("gen_ai.output.messages", FinalOutput);
    set("gen_ai.response.finish_reasons", """["stop"]""");
    set("gen_ai.usage.input_tokens", 24L);
    set("gen_ai.usage.output_tokens", 10L);
}

void AddCorrelation(Activity? activity)
{
    Set(activity, "session.id", SessionId);
    Set(activity, "user.id", UserId);
}

void Set(Activity? activity, string name, object? value)
{
    if (activity == null)
    {
        throw new InvalidOperationException($"span was not sampled while setting {name}");
    }
    activity.SetTag(name, value);
}

Uri BuildTraceEndpoint()
{
    var endpoint = (
        Environment.GetEnvironmentVariable("SIDESEAT_ENDPOINT")
        ?? "http://127.0.0.1:5388"
    ).TrimEnd('/');
    var project = Environment.GetEnvironmentVariable("SIDESEAT_PROJECT_ID") ?? "default";
    return new Uri($"{endpoint}/otel/{Uri.EscapeDataString(project)}/v1/traces");
}
