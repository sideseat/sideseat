"""API utilities for e2e tests."""

import json
from typing import Any
from urllib.error import HTTPError, URLError
from urllib.parse import quote
from urllib.request import Request, urlopen

from .config import API_BASE, API_CALL_TIMEOUT
from .logging import log_error


def api_call(
    endpoint: str,
    method: str = "GET",
    data: dict[str, Any] | None = None,
    base: str = API_BASE,
    timeout: int = API_CALL_TIMEOUT,
    expect_error: bool = False,
) -> dict[str, Any] | list[dict[str, Any]] | None:
    """Make an API call and return JSON response."""
    url = f"{base}{endpoint}"
    headers = {"Content-Type": "application/json"}

    try:
        body = json.dumps(data).encode() if data else None
        req = Request(url, data=body, headers=headers, method=method)
        with urlopen(req, timeout=timeout) as response:
            return json.loads(response.read().decode())
    except HTTPError as e:
        if expect_error:
            return {"error": str(e), "code": e.code}
        log_error(f"HTTP error: {url} - {e.code} {e.reason}")
        return None
    except URLError as e:
        if expect_error:
            return {"error": str(e)}
        log_error(f"URL error: {url} - {e}")
        return None
    except json.JSONDecodeError as e:
        log_error(f"Invalid JSON response: {e}")
        return None


def encode_param(value: str) -> str:
    """URL-encode a parameter value."""
    return quote(value, safe="")
