"""OTLP performance tests - load and query benchmarks."""

from .load import LoadTests
from .query import QueryTests

__all__ = [
    "LoadTests",
    "QueryTests",
]
