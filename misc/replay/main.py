"""
Replay OTLP debug files to the server.

Files are JSONL with format: {"timestamp":..., "project_id":..., "data":{...}}
The data field contains raw ExportTraceServiceRequest/Metrics/Logs.
"""

import argparse
import gzip
import json
import os
import re
import shutil
import sys
import tempfile
import zipfile
from pathlib import Path

import requests
from dotenv import load_dotenv
from tqdm import tqdm

# Load .env from samples directory
_samples_dir = Path(__file__).parent.parent
load_dotenv(_samples_dir / ".env")

FIXTURES_DIR = _samples_dir / "fixtures"
HEADERS = {"Content-Type": "application/json"}


def get_base_url() -> str:
    """Extract base URL from OTEL_EXPORTER_OTLP_ENDPOINT or use default."""
    endpoint = os.getenv(
        "OTEL_EXPORTER_OTLP_ENDPOINT", "http://127.0.0.1:5388/otel/default"
    )
    # Strip /otel/{project_id} suffix if present
    match = re.match(r"(https?://[^/]+)", endpoint)
    return match.group(1) if match else "http://127.0.0.1:5388"


def detect_signal_type(filename: str) -> str:
    """Detect signal type from filename."""
    name = filename.lower()
    if "metric" in name:
        return "metrics"
    if "log" in name:
        return "logs"
    return "traces"


def count_lines(filepath: Path) -> int:
    """Count non-empty lines efficiently."""
    count = 0
    with open(filepath, "rb") as f:  # binary mode for speed
        for line in f:
            if line.strip():
                count += 1
    return count


def replay_file(filepath: Path, base_url: str) -> tuple[int, int]:
    """Replay JSONL file to OTLP endpoint. Returns (sent, errors)."""
    signal = detect_signal_type(filepath.name)
    total = count_lines(filepath)
    sent, errors = 0, 0

    # Connection pooling via Session
    with requests.Session() as session:
        session.headers.update(HEADERS)

        with open(filepath, "r", encoding="utf-8") as f:
            for line in tqdm(f, total=total, desc=f"Replaying {filepath.name}"):
                line = line.strip()
                if not line:
                    continue

                try:
                    entry = json.loads(line)
                except json.JSONDecodeError as e:
                    errors += 1
                    tqdm.write(f"JSON error: {e}")
                    continue

                project_id = entry.get("project_id", "default")
                data = entry.get("data")
                if data is None:
                    errors += 1
                    tqdm.write("Missing 'data' field")
                    continue

                url = f"{base_url}/otel/{project_id}/v1/{signal}"

                try:
                    # Send data dict directly - requests handles serialization
                    resp = session.post(url, json=data, timeout=30)
                    resp.raise_for_status()
                    sent += 1
                except requests.RequestException as e:
                    errors += 1
                    tqdm.write(f"Request error: {e}")

    return sent, errors


ARCHIVE_SUFFIXES = {".gz", ".zip"}


def decompress_to_temp(path: Path, tmp_dir: Path) -> Path:
    """Decompress .gz or .zip archive into tmp_dir, return path to the JSONL file."""
    if path.suffix == ".gz":
        # e.g. traces.jsonl.gz -> traces.jsonl
        out = tmp_dir / path.stem
        with gzip.open(path, "rb") as f_in, open(out, "wb") as f_out:
            shutil.copyfileobj(f_in, f_out)
        return out

    if path.suffix == ".zip":
        with zipfile.ZipFile(path, "r") as zf:
            names = zf.namelist()
            jsonl_files = [n for n in names if n.endswith(".jsonl")]
            if not jsonl_files:
                raise ValueError(f"No .jsonl file found inside {path.name}")
            zf.extract(jsonl_files[0], tmp_dir)
            return tmp_dir / jsonl_files[0]

    raise ValueError(f"Unsupported archive format: {path.suffix}")


def main() -> int:
    parser = argparse.ArgumentParser(
        description="Replay OTLP debug files",
        epilog="Files are read from fixtures/ directory unless absolute path given.",
    )
    parser.add_argument(
        "filename",
        help="JSONL file (relative to fixtures/ or absolute path)",
    )
    parser.add_argument(
        "--base-url",
        help="Server base URL (default: from OTEL_EXPORTER_OTLP_ENDPOINT or http://127.0.0.1:5388)",
    )
    args = parser.parse_args()

    # Resolve path
    path = Path(args.filename)
    if not path.is_absolute():
        path = FIXTURES_DIR / args.filename

    if not path.exists():
        print(f"Error: File not found: {path}", file=sys.stderr)
        return 1

    is_archive = path.suffix in ARCHIVE_SUFFIXES
    if not is_archive and path.suffix != ".jsonl":
        print(f"Warning: Expected .jsonl/.gz/.zip file, got: {path.suffix}", file=sys.stderr)

    base_url = args.base_url or get_base_url()
    tmp_dir = None

    try:
        if is_archive:
            tmp_dir = Path(tempfile.mkdtemp(prefix="replay_"))
            replay_path = decompress_to_temp(path, tmp_dir)
            print(f"Decompressed: {path.name} -> {replay_path.name}")
        else:
            replay_path = path

        print(f"Base URL: {base_url}")
        print(f"File: {path}")
        print(f"Signal: {detect_signal_type(path.name)}")
        print()

        sent, errors = replay_file(replay_path, base_url)

        print()
        print(f"Done: {sent} sent, {errors} errors")

        return 1 if errors > 0 else 0
    finally:
        if tmp_dir and tmp_dir.exists():
            shutil.rmtree(tmp_dir)
            print(f"Cleaned up temp: {tmp_dir}")


if __name__ == "__main__":
    sys.exit(main())
