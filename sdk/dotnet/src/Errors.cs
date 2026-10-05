using System;

namespace SideSeat;

/// <summary>Base class for every exception the SDK throws.</summary>
public class SideSeatException : Exception
{
    /// <summary>Create the exception.</summary>
    public SideSeatException(string message)
        : base(message)
    {
    }
}

/// <summary>Settings are invalid, or conflict with the configuration already in effect.</summary>
public sealed class SideSeatConfigurationException : SideSeatException
{
    /// <summary>Create the exception.</summary>
    public SideSeatConfigurationException(string message)
        : base(message)
    {
    }
}

/// <summary>An integration that was requested explicitly is unknown or its package is not installed.</summary>
public sealed class SideSeatIntegrationException : SideSeatException
{
    /// <summary>Create the exception.</summary>
    public SideSeatIntegrationException(string message)
        : base(message)
    {
    }
}
