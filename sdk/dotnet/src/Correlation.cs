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

/// <summary>
/// A correlation scope: every activity started inside it, across <c>await</c>, belongs to a session
/// and, optionally, a user. Dispose it to restore the outer session and user. A nested scope
/// overrides only the values it names.
/// </summary>
/// <remarks>
/// <see cref="SideSeatClient.Session"/> creates one. An application that adds SideSeat to its own
/// pipeline with <c>AddSideSeat</c> has no client and constructs the scope directly.
/// </remarks>
public sealed class SideSeatSession : IDisposable
{
    private Correlation? _previous;
    private int _disposed;

    /// <summary>Enter a session scope.</summary>
    /// <exception cref="ArgumentException">An id is empty, which would merge unrelated conversations.</exception>
    public SideSeatSession(string sessionId, string? userId = null)
    {
        Enter(Required(sessionId, nameof(sessionId)), userId);
    }

    private SideSeatSession()
    {
    }

    /// <summary>A scope that may name only a user, or nothing, as a trace's correlation can.</summary>
    internal static SideSeatSession Partial(string? sessionId, string? userId)
    {
        var scope = new SideSeatSession();
        scope.Enter(sessionId == null ? null : Required(sessionId, nameof(sessionId)), userId);
        return scope;
    }

    private void Enter(string? sessionId, string? userId)
    {
        if (userId != null)
        {
            Required(userId, nameof(userId));
        }
        _previous = Correlation.Current.Value;
        Correlation.Current.Value = Correlation.Merge(sessionId, userId);
    }

    /// <inheritdoc />
    public void Dispose()
    {
        if (Interlocked.Exchange(ref _disposed, 1) == 0)
        {
            Correlation.Current.Value = _previous;
        }
    }

    private static string Required(string value, string name)
    {
        if (string.IsNullOrEmpty(value))
        {
            throw new ArgumentException($"The {name} must not be empty.", name);
        }
        return value;
    }
}
