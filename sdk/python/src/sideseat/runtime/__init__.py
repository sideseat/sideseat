"""SideSeat runtime presence/introspection client."""

from sideseat.runtime.adapters import (
    register_agent_inspector,
    register_graph_inspector,
    register_mcp_inspector,
    register_swarm_inspector,
)
from sideseat.runtime.client import RuntimeClient
from sideseat.runtime.protocol import (
    PROTOCOL_VERSION,
    Envelope,
    ErrorCode,
    RegistrationManifest,
    make_envelope,
    parse_envelope,
)

__all__ = [
    "PROTOCOL_VERSION",
    "Envelope",
    "ErrorCode",
    "RegistrationManifest",
    "RuntimeClient",
    "make_envelope",
    "parse_envelope",
    "register_agent_inspector",
    "register_graph_inspector",
    "register_mcp_inspector",
    "register_swarm_inspector",
]
