# Invoking the `strands_ws` agent

Run the sample in one terminal:

```bash
uv run --directory misc/samples/python/strands strands strands_ws --sideseat
```

Once the banner shows the agent registered, fire an AG-UI run from
another terminal:

```bash
curl -N -X POST \
  http://127.0.0.1:5388/api/v1/project/default/agents/weather/runs \
  -H 'content-type: application/json' \
  -d '{
    "thread_id": "t1",
    "run_id": "r1",
    "state": {},
    "messages": [{"id": "m1", "role": "user", "content": "3-day forecast for Berlin"}],
    "tools": [],
    "context": [],
    "forwarded_props": {}
  }'
```

You'll see two things at once:

1. The `curl` terminal streams an AG-UI Server-Sent Events response
   (`data: {...}\n\n` per event).
2. The sample terminal paints the same AG-UI stream through the rich
   console renderer (`RUN_STARTED`, streamed text, `TOOL_CALL_*`,
   `RUN_FINISHED`).

Cancel the run by `Ctrl-C`-ing the `curl` — the SDK's renderer prints
`[INTERRUPTED]` and Strands' `Agent.cancel()` aborts the loop.

Hit `curl` twice in parallel for the same agent and the second request
gets a 409 with `{"code": "agent_busy"}` — Strands serialises invocations
on a single agent instance, so SideSeat rejects up front.
