# Azure OpenAI (`AzureOpenAI` client)

```bash
uv run --locked --directory examples/python/azure-openai sample --list
uv run --locked --directory examples/python/azure-openai sample tool_use              # native telemetry
uv run --locked --directory examples/python/azure-openai sample tool_use --sideseat   # SideSeat SDK
```

The scenarios use the `openai` package's `AzureOpenAI` client the way an Azure application does: a
resource endpoint, an API version, and a deployment name in place of a model. Native mode is
OpenInference's OpenAI instrumentor on a plain OpenTelemetry provider; SideSeat mode replaces it with
`sideseat.init(integrations=["azure-openai"])`, which installs the same instrumentor.

**The model is the harness's fake OpenAI server, not Azure.** Every live suite runs on Amazon Bedrock
credentials, which cannot reach Azure OpenAI, and Bedrock does not serve the `AzureOpenAI` client's
deployment routes. The fake (`harness/fakes/openai.py`, started in-process) answers the shared prompts
deterministically on Azure's routes, so captures need no credentials and no cassette. Only the
client's transport is redirected: the endpoint keeps an `openai.azure.com` resource name, because
OpenInference reports Azure as the provider from the client's host.

Chat Completions carries every scenario except `reasoning`, which uses the Responses API because that
is where reasoning summaries are returned. OpenInference replaces a base64 image longer than 32,000
characters with `__REDACTED__` by default, so `files` records the image as a placeholder; the PDF is
kept. A provider client has no agents and no MCP client, so there is no `multi_agent` or `mcp_tools`
scenario.
