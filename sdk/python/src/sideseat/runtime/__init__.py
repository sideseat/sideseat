"""SideSeat runtime presence/introspection client."""

from sideseat.runtime.adapters import (
    register_agent_inspector,
    register_graph_inspector,
    register_mcp_inspector,
    register_swarm_inspector,
)
from sideseat.runtime.client import RuntimeClient
from sideseat.runtime.protocol import (
    Envelope,
    ErrorCode,
    PROTOCOL_VERSION,
    RegistrationManifest,
    make_envelope,
    parse_envelope,
)

__all__ = [
    "RuntimeClient",
    "RegistrationManifest",
    "Envelope",
    "ErrorCode",
    "PROTOCOL_VERSION",
    "make_envelope",
    "parse_envelope",
    "register_agent_inspector",
    "register_mcp_inspector",
    "register_swarm_inspector",
    "register_graph_inspector",
]
