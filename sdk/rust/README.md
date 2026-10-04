# SideSeat Rust SDK

OpenTelemetry for AI agents, configured for [SideSeat](https://sideseat.ai) in one call.

[![crates.io](https://img.shields.io/crates/v/sideseat)](https://crates.io/crates/sideseat)
[![Rust 1.94.1+](https://img.shields.io/badge/rust-1.94.1%2B-blue)](https://www.rust-lang.org/)
[![License: MIT](https://img.shields.io/badge/License-MIT-green.svg)](LICENSE)

```bash
npx sideseat          # a local server on http://127.0.0.1:5388
cargo add sideseat
```

```rust
use std::time::Duration;

use sideseat::{Options, Session, SpanOptions};

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let telemetry = sideseat::init(Options::new().service_name("travel-agent"))?;

    Session::new("conversation-42")
        .user("user-7")
        .scope(telemetry.trace("plan-trip", SpanOptions::new(), || async {
            Ok::<_, std::io::Error>(())
        }))
        .await?;

    telemetry.shutdown(Duration::from_secs(5));
    Ok(())
}
```

- `init` installs global tracer and meter providers that export over OTLP/HTTP, plus a logger provider
  for log appenders.
- A `Session` attributes every span started inside it - including spans libraries create through the
  global tracer - to a session and user. The values never leave the process as W3C baggage.
- `trace` starts a root span, `span` a child of the active one.
- `flush` and `shutdown` return whether everything was exported.

The crate implements the
[SideSeat SDK contract](https://github.com/sideseat/sideseat/blob/main/docs/engineering/sdk-contract.md);
a conformance program (`examples/sdk-conformance.rs`) proves that its telemetry reads back exactly as
the same conversation sent with plain OpenTelemetry.

Documentation: [sideseat.ai/docs/sdks/rust](https://sideseat.ai/docs/sdks/rust/).

## License

[MIT](LICENSE)
