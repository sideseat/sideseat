# Contributing to SideSeat

## Development Setup

1. Clone the repository
2. Run `make setup` to install dependencies
3. Run `make dev` to start development servers

## Project Structure

- `server/` - Rust backend (Axum framework)
- `web/` - React 19 frontend (Vite)
- `cli/` - NPM package for distribution
- `docs/` - Documentation site (Starlight)

## Development Workflow

```bash
make dev          # Start both servers with hot reload
make test         # Run all tests
make fmt          # Format code
make lint         # Lint code
make build        # Production build
```

## Architecture

SideSeat is a single Rust binary that embeds the frontend. In development,
the frontend runs via Vite dev server. In production, the Rust binary
serves static files from the embedded `web/dist` directory.

## Adding Features

1. Backend: Add modules in `server/src/`
2. Frontend: Add components in `web/src/components/`
3. Update tests in `server/tests/`

## Testing

- Unit tests: `cargo test`
- Integration tests: Located in `server/tests/integration/`
- E2E tests: Located in `server/tests/e2e/`

## Code Style

- Rust: `cargo fmt` and `cargo clippy`
- TypeScript: ESLint + Prettier
