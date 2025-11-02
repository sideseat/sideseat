# Git Hooks

This directory contains git hooks that can be installed to automate common tasks.

## Available Hooks

### pre-commit

Automatically runs `make fmt` to format all code (Rust and frontend) before each commit. If any files are formatted, they are automatically staged for the commit.

## Installation

### Option 1: Using Make (Recommended)

```bash
make setup-hooks
```

### Option 2: Using the setup script

```bash
./scripts/setup-hooks.sh
```

### Option 3: Manual installation

```bash
cp hooks/pre-commit .git/hooks/pre-commit
chmod +x .git/hooks/pre-commit
```

## Usage

Once installed, the hooks run automatically:

- **pre-commit**: Runs automatically when you execute `git commit`

If you need to skip a hook temporarily, use the `--no-verify` flag:

```bash
git commit --no-verify -m "message"
```

## Notes

- Git hooks are **not** tracked by git and must be installed locally by each developer
- The hooks are copied from this directory to `.git/hooks/` during installation
- To update a hook, modify it in this directory and run `make setup-hooks` again
