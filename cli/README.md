# SideSeat

AI development toolkit providing a proxy, observability, debugging, and optimization platform for AI applications.

## Features

- **AI Proxy** - Intercept and inspect AI API calls
- **OpenTelemetry Integration** - Full observability for AI workflows
- **MCP Support** - Debug Model Context Protocol interactions
- **Agent-to-Agent (A2A)** - Monitor multi-agent communications
- **Prompt Optimization** - Analyze and improve prompts
- **Web Dashboard** - Real-time monitoring and debugging UI

## Quick Start

### Run without installation:
```bash
npx sideseat
```

### Install globally:
```bash
npm install -g sideseat
sideseat
```

## Usage

```bash
# Start SideSeat with default settings
sideseat

# Start with custom port
sideseat --port 5001

# Show help
sideseat --help
```

Once started, open http://localhost:5001 in your browser to access the dashboard.

## Requirements

- **Node.js**: 20.19+ or 22.12+
- **Operating System**: macOS, Linux, or Windows

## Supported Platforms

| Platform | Architecture | Status |
|----------|-------------|--------|
| macOS | Intel (x64) | ✅ Supported |
| macOS | Apple Silicon (ARM64) | ✅ Supported |
| Linux | x64 | ✅ Supported |
| Linux | ARM64 | ✅ Supported |
| Windows | x64 | ✅ Supported |

## Configuration

SideSeat can be configured via:
- Command-line arguments
- Environment variables
- `.env` file

See `sideseat --help` for all available options.

## Documentation

For full documentation, visit: [github.com/spugachev/sideseat](https://github.com/spugachev/sideseat)

## License

GNU AFFERO GENERAL PUBLIC LICENSE v3.0

## Support

- **Issues**: [github.com/spugachev/sideseat/issues](https://github.com/spugachev/sideseat/issues)
- **Repository**: [github.com/spugachev/sideseat](https://github.com/spugachev/sideseat)
