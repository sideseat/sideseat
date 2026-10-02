"""Exceptions raised by the SideSeat SDK."""


class SideSeatError(Exception):
    """Base class for every error the SDK raises."""


class ConfigurationError(SideSeatError):
    """Settings are invalid, or conflict with the configuration already in effect."""


class IntegrationError(SideSeatError):
    """An integration that was requested explicitly cannot be installed."""
