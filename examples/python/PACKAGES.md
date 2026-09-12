# UV Guide

## Installation

Install uv if you don't have it yet:

**macOS/Linux:**

```bash
curl -LsSf https://astral.sh/uv/install.sh | sh
```

**Windows PowerShell:**

```powershell
powershell -c "irm https://astral.sh/uv/install.ps1 | iex"
```

## 1. Create a new project

```bash
uv init myproject
cd myproject
```

## 2. Add runtime packages

```bash
uv add fastapi uvicorn
```

## 3. Add dev tools

```bash
uv add -D ruff pytest
```

## 4. Run your application

```bash
uv run main.py
```

## 5. Later, sync the env from pyproject

```bash
uv sync
```

## 6. Lock dependencies if you want a reproducible build

```bash
uv lock
```

## 7. Export to requirements.txt if needed

```bash
uv export -o requirements.txt
```
