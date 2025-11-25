# Contributing to SideSeat

Thank you for your interest in contributing to SideSeat! We welcome contributions of all kinds: bug reports, feature requests, documentation improvements, and code contributions.

---

## Table of Contents

- [Code of Conduct](#code-of-conduct)
- [Getting Started](#getting-started)
- [Development Workflow](#development-workflow)
- [Reporting Bugs](#reporting-bugs)
- [Suggesting Features](#suggesting-features)
- [Submitting Pull Requests](#submitting-pull-requests)
- [Code Style](#code-style)
- [Contributor License Agreement](#contributor-license-agreement)

---

## Code of Conduct

Please be respectful and constructive in all interactions. We are committed to providing a welcoming and inclusive environment for everyone.

---

## Getting Started

### Prerequisites

- Rust 1.75+
- Node.js 20.19+ or 22.12+
- Make

### Setup

```bash
# Clone the repository
git clone https://github.com/spugachev/sideseat.git
cd sideseat

# Install dependencies
make setup

# Start development servers
make dev
```

### Project Structure

```
sideseat/
├── server/     # Rust backend (Axum)
├── web/        # React frontend (Vite)
├── cli/        # NPM distribution package
├── docs/       # Documentation site (Starlight)
└── samples/    # Example projects
```

---

## Development Workflow

```bash
make dev          # Start both servers with hot reload
make test         # Run all tests
make fmt          # Format code
make lint         # Lint code
make build        # Production build
make clean        # Clean build artifacts
```

### Architecture

SideSeat is a single Rust binary with an embedded frontend. During development, the frontend runs via Vite dev server with hot reload. In production, static files are served from the embedded `web/dist` directory.

---

## Reporting Bugs

Before reporting a bug:

1. Search [existing issues](https://github.com/spugachev/sideseat/issues) to avoid duplicates
2. Verify you're using the latest version (`sideseat --version`)

When creating a bug report, include:

- Clear, descriptive title
- Steps to reproduce the issue
- Expected vs actual behavior
- SideSeat version and OS
- Relevant logs (run with `SIDESEAT_LOG=debug` for verbose output)

Use our [bug report template](https://github.com/spugachev/sideseat/issues/new?template=bug_report.yml) for consistency.

---

## Suggesting Features

We welcome feature suggestions! Before submitting:

1. Check if the feature has already been requested
2. Consider if it aligns with the project's goals

Include in your feature request:

- Problem statement: What pain point does this solve?
- Proposed solution: How should it work?
- Alternatives considered: What other approaches did you evaluate?

Use our [feature request template](https://github.com/spugachev/sideseat/issues/new?template=feature_request.yml).

---

## Submitting Pull Requests

### Before You Start

1. Open an issue to discuss significant changes
2. Fork the repository and create a feature branch
3. Keep PRs focused on a single issue or feature

### PR Process

1. **Fork and clone** the repository
2. **Create a branch** with a descriptive name:
   ```bash
   git checkout -b fix/issue-123-description
   git checkout -b feature/new-feature-name
   ```
3. **Make your changes**, following our code style
4. **Write tests** for new functionality
5. **Run checks** before committing:
   ```bash
   make fmt
   make lint
   make test
   ```
6. **Commit** with clear messages referencing the issue:
   ```bash
   git commit -m "Fix authentication redirect loop (#123)"
   ```
7. **Push** and create a pull request
8. **Respond** to review feedback promptly

### What Makes a Good PR

- Solves one specific problem
- Includes tests for new functionality
- Updates documentation if needed
- Has a clear description explaining _why_ and _what_
- References related issues

---

## Code Style

### Rust

- Format with `cargo fmt`
- Lint with `cargo clippy`
- Follow standard Rust conventions
- Use meaningful variable and function names
- Add doc comments for public APIs

### TypeScript/React

- Format with Prettier (`npm run fmt`)
- Lint with ESLint (`npm run lint`)
- Use functional components with hooks
- Follow the existing component patterns

---

## Contributor License Agreement

By contributing to SideSeat, you agree to the following terms:

### License Grant

You irrevocably assign and transfer all worldwide right, title, and interest in the copyright of your contribution to **Sergey Pugachev**, including all intellectual property rights therein. This assignment ensures unified ownership and consistent licensing of the project.

### Patent License

You grant Sergey Pugachev, the project maintainers, and all downstream users a perpetual, worldwide, non-exclusive, royalty-free, irrevocable license under any patent claims you own or control which are necessarily infringed by your contribution.

### Representations and Warranties

By submitting a contribution, you represent and warrant that:

- Your contribution is your original work, or you have sufficient rights to submit it
- Your contribution does not violate any third-party intellectual property rights
- You have the legal authority to make this contribution and enter into this agreement
- Your contribution does not conflict with any other agreement or obligation you have

### Waiver

You waive any claims against Sergey Pugachev, project maintainers, contributors, and users of the project relating to your contribution, except as explicitly permitted by the project's license.

### Acknowledgment

Your contribution will be publicly available and may be redistributed under the project's license terms. This agreement cannot be revoked once the contribution is merged.

---

## Security Vulnerabilities

**Do not report security vulnerabilities through public issues.**

Please report security issues privately by emailing **security@sideseat.dev**. You will receive a response within 48 hours.

See [SECURITY.md](SECURITY.md) for our full security policy.

---

## Recognition

All contributors are recognized in our release notes. Thank you for helping make SideSeat better!
