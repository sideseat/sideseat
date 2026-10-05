# Browser Use

```bash
uvx browser-use install                                                              # Playwright's Chromium, once
uv run --locked --directory examples/python/browser-use sample --list
uv run --locked --directory examples/python/browser-use sample tool_use              # native telemetry
uv run --locked --directory examples/python/browser-use sample tool_use --sideseat   # SideSeat SDK
```

Every scenario runs a Browser Use `Agent` in Playwright's headless Chromium shell (`agents.py`). The
browser may only open the small travel site `travel_site.py` serves on `127.0.0.1:47811`
(`BROWSER_USE_SITE_PORT` moves it), and the built-in web search is left out, so no scenario reaches
the public internet. The shared tools are registered as custom actions.

Native mode is Laminar, which Browser Use instruments itself with, exporting over OTLP with no
Laminar project key. SideSeat mode replaces that with `sideseat.init(integrations=["browser-use"])`.

Browser Use's Bedrock classes do not suit current Claude models, so the default model is `claude`:
`ChatAnthropic` with adaptive thinking on an `AsyncAnthropicBedrock` client (`models.py`).

Screenshots are off (`use_vision=False`): they vary from run to run, and would make the native and
SDK runs of one conversation differ. For the same reason the capture tool pins the tab names and
Laminar span ids each browser and run mint afresh.
