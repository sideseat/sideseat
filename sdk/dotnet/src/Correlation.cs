using System;
using System.Diagnostics;
using System.Threading;
using OpenTelemetry;

namespace SideSeat;

/// <summary>The session and user a span belongs to.</summary>
internal sealed class Correlation
{
    internal static readonly AsyncLocal<Correlation?> Current = new AsyncLocal<Correlation?>();

    internal Correlation(string? sessionId, string? userId)
    {
        SessionId = sessionId;
        UserId = userId;
    }

    internal string? SessionId { get; }

    internal string? UserId { get; }

    internal static Correlation Merge(string? sessionId, string? userId)
    {
        var outer = Current.Value;
        return new Correlation(
            sessionId ?? outer?.SessionId,
            userId ?? outer?.UserId);
    }
}

/// <summary>
/// Stamps <c>session.id</c> and <c>user.id</c> on every activity started inside a correlation scope,
/// including activities a framework creates. The values live in an <see cref="AsyncLocal{T}"/>, not
/// in W3C baggage, which HTTP instrumentation would forward to model providers.
/// </summary>
internal sealed class CorrelationProcessor : BaseProcessor<Activity>
{
    public override void OnStart(Activity data)
    {
        var correlation = Correlation.Current.Value;
        if (correlation?.SessionId != null)
        {
            data.SetTag("session.id", correlation.SessionId);
        }
        if (correlation?.UserId != null)
        {
            data.SetTag("user.id", correlation.UserId);
        }
    }
}

/// <summary>A correlation scope. Dispose it to restore the outer session and user.</summary>
public sealed class SideSeatSession : IDisposable
{
    private readonly object? _previous;
    private int _disposed;

    internal SideSeatSession(string? sessionId, string? userId)
    {
        _previous = Correlation.Current.Value;
        Correlation.Current.Value = Correlation.Merge(sessionId, userId);
    }

    /// <inheritdoc />
    public void Dispose()
    {
        if (Interlocked.Exchange(ref _disposed, 1) == 0)
        {
            Correlation.Current.Value = (Correlation?)_previous;
        }
    }
}
