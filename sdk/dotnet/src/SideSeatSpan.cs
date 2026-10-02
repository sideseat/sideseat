using System;
using System.Diagnostics;
using System.Threading;

namespace SideSeat;

/// <summary>An active SideSeat span. Dispose it to end the span and restore the previous context.</summary>
public sealed class SideSeatSpan : IDisposable
{
    private readonly Action? _restore;
    private int _disposed;

    internal SideSeatSpan(Activity? activity, Action? restore)
    {
        Activity = activity;
        _restore = restore;
    }

    /// <summary>The underlying activity, or null when telemetry is disabled.</summary>
    public Activity? Activity { get; }

    /// <summary>Add or replace an attribute.</summary>
    public SideSeatSpan SetAttribute(string name, object? value)
    {
        Activity?.SetTag(name, value);
        return this;
    }

    /// <summary>Record an exception and mark the span as failed.</summary>
    public SideSeatSpan RecordException(Exception exception)
    {
        if (exception == null)
        {
            throw new ArgumentNullException(nameof(exception));
        }
        Activity?.SetStatus(ActivityStatusCode.Error, exception.Message);
        Activity?.AddEvent(new ActivityEvent(
            "exception",
            tags: new ActivityTagsCollection
            {
                { "exception.type", exception.GetType().FullName ?? exception.GetType().Name },
                { "exception.message", exception.Message },
                { "exception.stacktrace", exception.ToString() },
            }));
        return this;
    }

    /// <inheritdoc />
    public void Dispose()
    {
        if (Interlocked.Exchange(ref _disposed, 1) != 0)
        {
            return;
        }
        Activity?.Stop();
        _restore?.Invoke();
    }
}
