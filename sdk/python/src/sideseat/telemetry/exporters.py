"""Custom span exporters."""

import json
import logging
import threading
from collections.abc import Sequence

from opentelemetry.sdk.trace import ReadableSpan
from opentelemetry.sdk.trace.export import SpanExporter, SpanExportResult

from sideseat.telemetry.encoding import span_to_dict

logger = logging.getLogger("sideseat.telemetry.exporters")


class JsonFileSpanExporter(SpanExporter):
    """Exports spans to JSONL file with base64-encoded binaries."""

    def __init__(self, path: str, mode: str = "a") -> None:
        if mode not in ("a", "w"):
            raise ValueError(f"mode must be 'a' or 'w', got {mode!r}")
        self._path = path
        self._lock = threading.Lock()
        self._closed = False
        self._fh = open(path, mode, encoding="utf-8")  # noqa: SIM115

    def export(self, spans: Sequence[ReadableSpan]) -> SpanExportResult:
        try:
            with self._lock:
                if self._closed:
                    return SpanExportResult.FAILURE
                for span in spans:
                    self._fh.write(json.dumps(span_to_dict(span), ensure_ascii=False))
                    self._fh.write("\n")
                self._fh.flush()
            return SpanExportResult.SUCCESS
        except Exception:
            logger.exception("Failed to export spans to %s", self._path)
            return SpanExportResult.FAILURE

    def shutdown(self) -> None:
        with self._lock:
            if self._closed:
                return
            self._fh.flush()
            self._fh.close()
            self._closed = True

    def force_flush(self, timeout_millis: int = 30000) -> bool:
        with self._lock:
            if self._closed:
                return False
            self._fh.flush()
        return True
