"""Where each scenario's truth comes from: a cassette, a fake model's script, or a fixed program.

* A suite on a live model has a cassette per scenario; its responses are decoded as recorded.
* A suite on a ``fake-*`` model has no cassette: the fake's answers are a deterministic function of
  the request (:mod:`harness.fakes.script`). The scenario is replayed here against the fake's own
  response builders, with the request a framework would send modelled from the scenario, and the
  responses are decoded through the same decoders as a cassette's.
* The SDK conformance programs emit one fixed conversation, read from the Python program.
* A coding-agent CLI (``examples/cli``) has a cassette per scenario like a live suite, and its prompts
  are the CLI driver's own, since a CLI scenario reads files and runs commands rather than calling the
  shared tools.
"""

from __future__ import annotations

import functools
import hashlib
import importlib.util
import json
import re
import sys
from dataclasses import dataclass
from pathlib import Path
from typing import Any

from harness import capture, catalog, content, models, tooling
from harness.fakes import anthropic as fake_anthropic
from harness.fakes import google_genai as fake_gemini
from harness.fakes import openai as fake_openai
from harness.scrub import PLACEHOLDER_USER
from harness.truth import derive
from harness.truth.derive import Builder, Framework, Options, assemble, document
from harness.truth import request_truth
from harness.truth.wire import ModelCall, decode, decode_cassette

REPO = derive.REPO
FIXTURES = capture.FIXTURES
TRUTH = REPO / "server" / "tests" / "fixtures" / "truth"
CONFORMANCE_PROGRAM = (
    REPO / "examples" / "python" / "sdk-conformance" / "conformance.py"
)
CLI_DRIVER = REPO / "examples" / "cli" / "capture.py"
#: Producers whose fixtures come from the conformance programs, one per SDK language.
CONFORMANCE_PRODUCERS = ("python", "javascript", "dotnet", "rust")
CONFORMANCE_SOURCES = {
    "python": "examples/python/sdk-conformance/conformance.py",
    "javascript": "examples/javascript/sdk-conformance/conformance.ts",
    "dotnet": "examples/dotnet/conformance/Program.cs",
    "rust": "sdk/rust/examples/sdk-conformance.rs",
}


class Underivable(Exception):
    """A scenario whose truth cannot be derived offline, with the reason."""


@dataclass(frozen=True)
class Target:
    producer: str
    scenario: str


#: Captures no truth describes, with the reason: a mode for every producer, or one producer's mode. Each is
#: left out of every derived document's fixture list and reported as uncovered, so "this has no truth" is one
#: statement both sides read - `message_truth::truth::reason_without_truth` is the same rule on the Rust side.
#:
#: A mode is not uncoverable in itself: `claude-code/logs` is covered, because that CLI's log records carry a
#: span per model call. Whether a channel can be held to the per-call truth is a fact about the producer.
UNCOVERED_MODES = {
    "legacy": "a pre-catalog capture with no cassette or script",
    # This dialect logs one event per model call but attaches every record to the turn's span, so the truth's
    # per-call checks have nothing to key on. The goldens hold the conversation; the declaration of what the
    # channel cannot carry is the rubric's to write, and this entry goes when it lands.
    "autogen/logs": "the channel reports no span per model call, so the per-call truth has nothing to key on",
}


def uncovered(producer: str, mode: str) -> str | None:
    """Why no truth describes this producer's mode, if none may."""
    return UNCOVERED_MODES.get(f"{producer}/{mode}") or UNCOVERED_MODES.get(mode)


def fixtures_of(producer: str, scenario: str) -> list[str]:
    root = FIXTURES / producer
    if not root.is_dir():
        return []
    return [
        f"{producer}/{mode.name}/{scenario}"
        for mode in sorted(root.iterdir())
        if (mode / scenario).is_dir() and uncovered(producer, mode.name) is None
    ]


def _relative(path: Path) -> str:
    return path.resolve().relative_to(REPO).as_posix()


def _sha256(path: Path) -> str:
    return hashlib.sha256(path.read_bytes()).hexdigest()


def targets() -> list[Target]:
    """Every producer/scenario a truth is attempted for: suites' scenarios and the conformance runs."""
    found = []
    for producer, suite in sorted(capture.suites().items()):
        for scenario in suite_scenarios(suite):
            found.append(Target(producer, scenario))
    found += [Target(producer, "canonical") for producer in CONFORMANCE_PRODUCERS]
    found += [
        Target(producer, scenario)
        for producer in sorted(_cli_driver().CLIS)
        for scenario in _cli_driver().SCENARIOS
    ]
    return found


def _unexported(builder: Builder, unexported: dict[str, str]) -> None:
    """Withdraw the assertions a CLI's telemetry cannot satisfy, each with the CLI's own reason."""
    finals = {
        fact
        for conversation in builder.conversations
        for fact in conversation["final_answers"]
    }
    for fact in builder.facts:
        kind = fact["kind"]
        if kind not in unexported or (kind == "text" and fact["id"] in finals):
            continue
        fact["require"] = None
        builder.gap(kind, "not_exported", unexported[kind], subject=fact["id"])


@functools.cache
def _cli_driver() -> Any:
    """The CLI capture driver, which declares the CLIs and the prompts of each scenario."""
    spec = importlib.util.spec_from_file_location("cli_capture", CLI_DRIVER)
    assert spec and spec.loader
    module = importlib.util.module_from_spec(spec)
    # Registered before it runs: its dataclasses resolve their module through `sys.modules`.
    sys.modules[spec.name] = module
    spec.loader.exec_module(module)
    return module


def cli(target: Target) -> dict[str, Any]:
    """A CLI scenario's truth: its cassette, laid out against the prompts the driver sent."""
    driver = _cli_driver()
    cassette = (
        CLI_DRIVER.parent / target.producer / "cassettes" / f"{target.scenario}.json"
    )
    if not cassette.exists():
        raise Underivable(f"no cassette at {_relative(cassette)}")
    calls, ignored = decode_cassette(cassette)
    if not calls:
        raise Underivable(f"{_relative(cassette)} records no model call")
    turns = driver.SCENARIOS[target.scenario].turns
    builder = assemble(target.producer, target.scenario, calls, prompts=[tuple(turns)])
    _unexported(builder, driver.CLIS[target.producer].unexported)
    source = {
        "kind": "cassette",
        "path": _relative(cassette),
        "sha256": _sha256(cassette),
        "non_model_requests": ignored,
    }
    return document(
        builder,
        fixtures=fixtures_of(target.producer, target.scenario),
        source=source,
        request_bodies="unrecorded",
    )


def suite_scenarios(suite: capture.Suite) -> list[str]:
    """The catalog scenarios a suite implements, read from its files rather than by running it.

    A Go or JVM suite registers its scenarios in code rather than one file each, so it is asked, the way
    capture asks it, with ``--list``.
    """
    if suite.language == "python":
        names = {p.stem for p in (suite.root / "scenarios").glob("*.py")}
    elif suite.language == "javascript":
        names = {p.stem for p in (suite.root / "scenarios").glob("*.ts")}
    else:
        names = set(capture.scenarios_of(suite))
    return [name for name in catalog.CATALOG if name in names]


def build(target: Target) -> dict[str, Any]:
    document_ = _build(target)
    cassette = _cassette_of(target)
    interaction_calls = (
        {
            index: f"call-{n:03d}"
            for n, index in enumerate(
                request_truth.model_interactions(cassette), start=1
            )
        }
        if cassette is not None
        else None
    )
    requests = {
        fixture: recorded
        for fixture in document_["fixtures"]
        if (
            recorded := request_truth.fixture_requests(
                document_, fixture, interaction_calls
            )
        )
        is not None
    }
    document_.pop(request_truth.THREADS, None)
    if requests:
        document_["requests"] = requests
    return document_


def _cassette_of(target: Target) -> Path | None:
    """The cassette that answered a scenario's model calls, if one did."""
    if target.producer in _cli_driver().CLIS:
        path = (
            CLI_DRIVER.parent
            / target.producer
            / "cassettes"
            / f"{target.scenario}.json"
        )
        return path if path.exists() else None
    suite = capture.suites().get(target.producer)
    if suite is None or capture.uses_fake_model(
        suite, suite.model_for(target.scenario, None)
    ):
        return None
    path = suite.root / "cassettes" / f"{target.scenario}.json"
    return path if path.exists() else None


def _build(target: Target) -> dict[str, Any]:
    if target.scenario == "canonical" and target.producer in CONFORMANCE_PRODUCERS:
        return conformance(target.producer)
    if target.producer in _cli_driver().CLIS:
        return cli(target)
    suite = capture.suites().get(target.producer)
    if suite is None:
        raise Underivable(f"no suite produces {target.producer!r}")
    if target.scenario not in derive.PROMPTS:
        raise Underivable(f"{target.scenario!r} is not a catalog scenario")
    fixtures = fixtures_of(target.producer, target.scenario)
    model = suite.model_for(target.scenario, None)
    if capture.uses_fake_model(suite, model):
        surface = models.resolve(model or suite.manifest["default-model"]).surface
        return fake(
            target, surface, fixtures, Framework.of(suite.manifest.get("truth"))
        )
    cassette = suite.root / "cassettes" / f"{target.scenario}.json"
    if not cassette.exists():
        raise Underivable(f"no cassette at {_relative(cassette)}")
    calls, ignored = decode_cassette(cassette)
    if not calls:
        raise Underivable(f"{_relative(cassette)} records no model call")
    builder = assemble(
        target.producer,
        target.scenario,
        calls,
        options=Options(framework=Framework.of(suite.manifest.get("truth"))),
    )
    source = {
        "kind": "cassette",
        "path": _relative(cassette),
        "sha256": _sha256(cassette),
        "non_model_requests": ignored,
    }
    return document(
        builder, fixtures=fixtures, source=source, request_bodies="unrecorded"
    )


# --- Fake models --------------------------------------------------------------------------------

#: The tools each scenario declares, as the shared scripts define them.
SCENARIO_TOOLS = {
    "tool_use": ("get_weather", "get_precipitation"),
    "streaming": ("get_weather",),
    "error": ("book_flight",),
    "mcp_tools": ("calculate",),
    "trailing_tool": ("get_weather",),
}
#: The tools the provider runs itself that each scenario enables; the Responses API is the one that
#: offers them.
SCENARIO_HOSTED_TOOLS = {
    "server_tools": ("web_search",),
}
CALCULATOR = {
    "type": "object",
    "properties": {"expression": {"type": "string"}},
    "required": ["expression"],
}


def _tool_schemas(scenario: str) -> dict[str, dict[str, Any]]:
    schemas = {}
    for name in SCENARIO_TOOLS.get(scenario, ()):
        if name == "calculate":
            schemas[name] = CALCULATOR
        else:
            schemas[name] = tooling.spec(getattr(content, name)).parameters
    return schemas


def _result_text(name: str, arguments: Any) -> str:
    outcome = derive.run_tool(name, arguments)
    if outcome is None:
        raise Underivable(f"the fake model called {name}, which is not a shared tool")
    if outcome.is_error:
        return f"{content.BookingUnavailable.__name__}: {outcome.value}"
    value = outcome.value
    return value if isinstance(value, str) else json.dumps(value)


def _interaction(path: str, body: bytes, streamed: bool) -> dict[str, Any]:
    return {
        "method": "POST",
        "path": path,
        "status": 200,
        "headers": {
            "content-type": "text/event-stream" if streamed else "application/json"
        },
        "body": body,
    }


def _sse(events: list[dict[str, Any]]) -> bytes:
    return "".join(f"data: {json.dumps(event)}\n\n" for event in events).encode()


class _OpenAiChat:
    """The Chat Completions requests a framework sends a fake OpenAI model, modelled."""

    def __init__(self, scenario: str, model: str) -> None:
        self.scenario, self.model = scenario, model
        self.messages: list[dict[str, Any]] = []

    def user(self, text: str) -> None:
        self.messages.append({"role": "user", "content": text})

    def call(self) -> tuple[dict[str, Any], ModelCall]:
        body: dict[str, Any] = {"model": self.model, "messages": self.messages}
        if tools := _tool_schemas(self.scenario):
            body["tools"] = [
                {"type": "function", "function": {"name": name, "parameters": schema}}
                for name, schema in tools.items()
            ]
        if self.scenario == "structured_output":
            body["response_format"] = {
                "type": "json_schema",
                "json_schema": {
                    "name": "TripPlan",
                    "schema": content.TripPlan.model_json_schema(),
                },
            }
        streamed = self.scenario == "streaming"
        if streamed:
            payload = _sse(fake_openai.completion_chunks({**body, "stream": True}))
        else:
            payload = json.dumps(fake_openai.completion(body)).encode()
        call = decode(_interaction("/v1/chat/completions", payload, streamed))
        assert call is not None
        return body, call

    def answer(self, call: ModelCall) -> None:
        message: dict[str, Any] = {
            "role": "assistant",
            "content": "".join(p["text"] for p in call.parts if p["type"] == "text")
            or None,
        }
        if call.tool_calls:
            message["tool_calls"] = [
                {
                    "id": part["id"],
                    "type": "function",
                    "function": {
                        "name": part["name"],
                        "arguments": json.dumps(part["arguments"]),
                    },
                }
                for part in call.tool_calls
            ]
        self.messages.append(message)
        for part in call.tool_calls:
            self.messages.append(
                {
                    "role": "tool",
                    "tool_call_id": part["id"],
                    "content": _result_text(part["name"], part["arguments"]),
                }
            )


class _OpenAiResponses(_OpenAiChat):
    """The Responses API requests, used where a scenario needs reasoning summaries or the provider's
    own tools."""

    def user(self, text: str) -> None:
        self.messages.append({"type": "message", "role": "user", "content": text})

    def call(self) -> tuple[dict[str, Any], ModelCall]:
        body: dict[str, Any] = {"model": self.model, "input": self.messages}
        if self.scenario == "reasoning":
            body["reasoning"] = {"effort": "high", "summary": "detailed"}
        if hosted := SCENARIO_HOSTED_TOOLS.get(self.scenario):
            body["tools"] = [{"type": tool} for tool in hosted]
        payload = json.dumps(fake_openai.response(body)).encode()
        call = decode(_interaction("/v1/responses", payload, False))
        assert call is not None
        return body, call

    def answer(self, call: ModelCall) -> None:
        for part in call.parts:
            if part["type"] == "text":
                self.messages.append(
                    {"type": "message", "role": "assistant", "content": part["text"]}
                )
            elif part["type"] == "tool_call":
                self.messages.append(
                    {
                        "type": "function_call",
                        "call_id": part["id"],
                        "name": part["name"],
                        "arguments": json.dumps(part["arguments"]),
                    }
                )
        for part in call.tool_calls:
            self.messages.append(
                {
                    "type": "function_call_output",
                    "call_id": part["id"],
                    "output": _result_text(part["name"], part["arguments"]),
                }
            )


class _Anthropic:
    """The Messages API requests the Anthropic client sends a fake Claude, modelled."""

    def __init__(self, scenario: str, model: str) -> None:
        self.scenario, self.model = scenario, model
        self.messages: list[dict[str, Any]] = []

    def user(self, text: str) -> None:
        self.messages.append({"role": "user", "content": text})

    def call(self) -> tuple[dict[str, Any], ModelCall]:
        body: dict[str, Any] = {
            "model": self.model,
            "max_tokens": 1024,
            "messages": self.messages,
        }
        tools: list[dict[str, Any]] = [
            {"name": name, "input_schema": schema}
            for name, schema in _tool_schemas(self.scenario).items()
        ]
        # The direct release of each tool the provider runs itself.
        tools += [
            {"type": f"{tool}_20250305", "name": tool}
            for tool in SCENARIO_HOSTED_TOOLS.get(self.scenario, ())
        ]
        if tools:
            body["tools"] = tools
        if self.scenario == "structured_output":
            schema = content.TripPlan.model_json_schema()
            body["output_config"] = {
                "format": {"type": "json_schema", "schema": schema}
            }
        if self.scenario == "reasoning":
            body["thinking"] = {"type": "adaptive"}
        message = fake_anthropic.message(body)
        streamed = self.scenario == "streaming"
        if streamed:
            payload = _sse([event for _, event in fake_anthropic.events_of(message)])
        else:
            payload = json.dumps(message).encode()
        call = decode(_interaction("/v1/messages", payload, streamed))
        assert call is not None
        return body, call

    def answer(self, call: ModelCall) -> None:
        blocks: list[dict[str, Any]] = []
        for part in call.parts:
            if part["type"] == "text":
                blocks.append({"type": "text", "text": part["text"]})
            elif part["type"] == "tool_call":
                blocks.append(
                    {
                        "type": "tool_use",
                        "id": part["id"],
                        "name": part["name"],
                        "input": part["arguments"],
                    }
                )
        self.messages.append({"role": "assistant", "content": blocks})
        if call.tool_calls:
            self.messages.append(
                {
                    "role": "user",
                    "content": [
                        {
                            "type": "tool_result",
                            "tool_use_id": part["id"],
                            "content": _result_text(part["name"], part["arguments"]),
                        }
                        for part in call.tool_calls
                    ],
                }
            )


class _Gemini:
    """The generateContent requests the Google Gen AI SDK sends a fake Gemini model, modelled."""

    def __init__(self, scenario: str, model: str) -> None:
        self.scenario, self.model = scenario, model
        self.contents: list[dict[str, Any]] = []

    def user(self, text: str) -> None:
        self.contents.append({"role": "user", "parts": [{"text": text}]})

    def call(self) -> tuple[dict[str, Any], ModelCall]:
        body: dict[str, Any] = {"contents": self.contents}
        if tools := _tool_schemas(self.scenario):
            body["tools"] = [
                {
                    "functionDeclarations": [
                        {"name": name, "parametersJsonSchema": schema}
                        for name, schema in tools.items()
                    ]
                }
            ]
        config: dict[str, Any] = {}
        if self.scenario == "structured_output":
            config["responseJsonSchema"] = content.TripPlan.model_json_schema()
        if self.scenario == "reasoning":
            config["thinkingConfig"] = {"includeThoughts": True}
        if config:
            body["generationConfig"] = config
        parts = fake_gemini.parts_of(
            fake_gemini.script.reply(fake_gemini.request_of(body))
        )
        response_id = "gemini-" + fake_gemini.script.digest(body)
        streamed = self.scenario == "streaming"
        if streamed:
            payload = _sse(fake_gemini.stream_of(parts, response_id))
            path = f"/v1beta/models/{self.model}:streamGenerateContent"
        else:
            payload = json.dumps(
                fake_gemini.chunk(parts, final=True, response_id=response_id)
            ).encode()
            path = f"/v1beta/models/{self.model}:generateContent"
        call = decode(_interaction(path, payload, streamed))
        assert call is not None
        return body, call

    def answer(self, call: ModelCall) -> None:
        parts: list[dict[str, Any]] = []
        for part in call.parts:
            if part["type"] == "text":
                parts.append({"text": part["text"]})
            elif part["type"] == "tool_call":
                parts.append(
                    {
                        "functionCall": {
                            "id": part["id"],
                            "name": part["name"],
                            "args": part["arguments"],
                        }
                    }
                )
        self.contents.append({"role": "model", "parts": parts})
        if call.tool_calls:
            self.contents.append(
                {
                    "role": "user",
                    "parts": [
                        {
                            "functionResponse": {
                                "id": part["id"],
                                "name": part["name"],
                                "response": {
                                    "result": _result_text(
                                        part["name"], part["arguments"]
                                    )
                                },
                            }
                        }
                        for part in call.tool_calls
                    ],
                }
            )


def fake_calls(surface: str, scenario: str) -> list[ModelCall]:
    """The responses a fake model gives the scenario, decoded."""
    if scenario == "multi_agent":
        raise Underivable(
            "the framework declares its own handoff tools and agent prompts, which the fake answers "
            "from their schemas; without the recorded requests the conversation cannot be modelled"
        )
    model = next(m.id for m in models.MODELS.values() if m.surface == surface)
    calls: list[ModelCall] = []
    for group in derive.PROMPTS[scenario]:
        if surface == "fake-gemini":
            session: Any = _Gemini(scenario, fake_gemini.MODEL_VERSION)
        elif surface == "fake-openai" and (
            scenario == "reasoning" or scenario in SCENARIO_HOSTED_TOOLS
        ):
            session = _OpenAiResponses(scenario, model)
        elif surface == "fake-openai":
            session = _OpenAiChat(scenario, model)
        elif surface == "fake-anthropic":
            session = _Anthropic(scenario, model)
        else:
            raise Underivable(f"no request model for {surface}")
        for prompt in group:
            session.user(prompt)
            for _ in range(8):
                _, call = session.call()
                calls.append(call)
                session.answer(call)
                if not call.tool_calls:
                    break
            else:
                raise Underivable("the fake model did not converge on an answer")
    return calls


def fake(
    target: Target,
    surface: str,
    fixtures: list[str],
    framework: Framework | None = None,
) -> dict[str, Any]:
    calls = fake_calls(surface, target.scenario)
    builder = assemble(
        target.producer,
        target.scenario,
        calls,
        options=Options(
            metadata_from_wire=False,
            answers_follow_results=True,
            framework=framework or Framework(),
        ),
    )
    for record in builder.calls:
        if record["api"] != "gemini.generate_content":
            # The fake echoes the model the request named, which the framework chose.
            record["response_model"] = None
    builder.gap(
        "request",
        "request_modelled",
        "a fake model answers the request it receives, and that request was not recorded: the one "
        "modelled here declares the scenario's shared tools and schema as the scripts do, so a "
        "fact is only as certain as that model of the framework's request",
    )
    builder.gap(
        "model_metadata",
        "fake_model_echoes_request",
        "a fake model echoes the request's model and derives response ids from the request body; "
        "neither is recorded, so neither is asserted",
    )
    source = {
        "kind": "fake-script",
        "surface": surface,
        "path": "examples/python/harness/harness/fakes/script.py",
        "sha256": _sha256(REPO / "examples/python/harness/harness/fakes/script.py"),
    }
    return document(
        builder, fixtures=fixtures, source=source, request_bodies="modelled"
    )


# --- SDK conformance ----------------------------------------------------------------------------


def _conformance_module() -> Any:
    spec = importlib.util.spec_from_file_location(
        "sideseat_conformance", CONFORMANCE_PROGRAM
    )
    assert spec is not None and spec.loader is not None
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    return module


def _attributes(add: Any) -> dict[str, Any]:
    attributes: dict[str, Any] = {}
    add(attributes.__setitem__)
    return attributes


def conformance(producer: str) -> dict[str, Any]:
    """The canonical conversation every SDK conformance program emits.

    The Python program is read for its values; the other languages' programs must carry the same
    literals, which is checked here so a drifted program fails truth generation rather than
    silently sharing a truth that no longer describes it.
    """
    program = _conformance_module()
    first = _attributes(program.add_first_model_call)
    tool = _attributes(program.add_tool_call)
    final = _attributes(program.add_final_model_call)
    question = json.loads(first["gen_ai.input.messages"])[0]["parts"][0]["content"]
    first_text = json.loads(first["gen_ai.output.messages"])[0]["parts"][0]["content"]
    final_text = json.loads(final["gen_ai.output.messages"])[0]["parts"][0]["content"]
    source_path = REPO / CONFORMANCE_SOURCES[producer]
    text = source_path.read_text()
    literals = [
        question,
        first_text,
        final_text,
        first["gen_ai.request.model"],
        tool["gen_ai.tool.call.id"],
        tool["gen_ai.tool.name"],
        tool["gen_ai.tool.call.arguments"],
        tool["gen_ai.tool.call.result"],
    ]
    numbers = [
        first["gen_ai.usage.input_tokens"],
        first["gen_ai.usage.output_tokens"],
        final["gen_ai.usage.input_tokens"],
        final["gen_ai.usage.output_tokens"],
    ]
    # A number may carry a language's literal suffix: `6L` in C#, `24_i64` in Rust.
    missing = [repr(literal) for literal in literals if literal not in text]
    missing += [
        str(n)
        for n in numbers
        if not re.search(rf"(?<![\w.]){n}(?:_?[iuL]\w*)?(?![\w.])", text)
    ]
    if missing:
        raise Underivable(
            f"{CONFORMANCE_SOURCES[producer]} no longer carries {', '.join(missing)}"
        )

    builder = Builder(producer, "canonical")
    conversation = builder.conversation()
    exact_conversation = derive._conversation_requirement("exact")
    prompt = builder.fact(
        conversation,
        "user_text",
        "user",
        "program",
        {"text": question},
        require=exact_conversation,
    )

    def model_call(attributes: dict[str, Any], text_value: str, finish: str) -> str:
        identifier = f"call-{len(builder.calls) + 1:03d}"
        fact = builder.fact(
            conversation,
            "text",
            "assistant",
            "program",
            {"text": text_value},
            call=identifier,
            require=derive._model_call_requirement(),
        )
        builder.calls.append(
            {
                "id": identifier,
                "attempt": 1,
                "outcome": "success",
                "conversation": conversation["id"],
                "api": "conformance",
                "streamed": False,
                "status": 200,
                "model": attributes["gen_ai.request.model"],
                "response_model": None,
                "response_id": None,
                "stop_reason": None,
                "finish": finish,
                "usage": {
                    "input": attributes["gen_ai.usage.input_tokens"],
                    "output": attributes["gen_ai.usage.output_tokens"],
                    "cache_read": None,
                    "cache_write": None,
                    "reasoning": None,
                    "input_includes_cache": False,
                },
                "outputs": [fact],
            }
        )
        return identifier

    first_call = model_call(first, first_text, "tool_use")
    builder.edges.append({"kind": "prompt_of", "from": prompt, "to": first_call})
    # The program reports the call on its tool span only, not in the first call's output.
    call_fact = builder.fact(
        conversation,
        "tool_call",
        "assistant",
        "program",
        {
            "id": tool["gen_ai.tool.call.id"],
            "name": tool["gen_ai.tool.name"],
            "arguments": json.loads(tool["gen_ai.tool.call.arguments"]),
        },
        require=derive._conversation_requirement("semantic"),
    )
    result_fact = builder.fact(
        conversation,
        "tool_result",
        "tool",
        "program",
        {
            "call_id": tool["gen_ai.tool.call.id"],
            "name": tool["gen_ai.tool.name"],
            "value": json.loads(tool["gen_ai.tool.call.result"]),
            "is_error": False,
        },
        require=derive._conversation_requirement("semantic"),
    )
    builder.edges.append({"kind": "result_of", "from": result_fact, "to": call_fact})
    final_call = model_call(final, final_text, "stop")
    builder.edges.append(
        {"kind": "call_order", "before": first_call, "after": final_call}
    )
    conversation["final_answers"] = [builder.calls[-1]["outputs"][0]]
    source = {
        "kind": "program",
        "path": CONFORMANCE_SOURCES[producer],
        "sha256": _sha256(source_path),
        "values_from": _relative(CONFORMANCE_PROGRAM),
    }
    return document(
        builder,
        fixtures=fixtures_of(producer, "canonical"),
        source=source,
        request_bodies="program",
    )


# --- Output -------------------------------------------------------------------------------------


def path_of(target: Target) -> Path:
    return TRUTH / target.producer / f"{target.scenario}.json"


#: An account name in a home-directory path, or in an identifier a framework derived from one (CrewAI
#: names an MCP tool after the command path that started it: ``..._users_<name>_<hash>``).
_HOME = re.compile(r"(/Users/|/home/)([^/\s\"]+)")
_IDENTIFIER = re.compile(r"(_users_)([A-Za-z0-9.-]+)(?=_)")


def anonymise(text: str) -> str:
    """Replaces an account name with the placeholder the fixtures use, whoever regenerates the truth.

    The proxy scrubs a cassette only of the account that recorded it, so a model's echo of a path in
    a cassette recorded elsewhere can still name another account.
    A path gets the placeholder itself, which the repository's home-directory sweep accepts. An
    identifier gets it cut or padded to the name's length, as the fixture capture does - its payloads
    are length-prefixed protobuf - so the truth names a tool exactly as the fixture does.
    """
    placeholder = PLACEHOLDER_USER.decode()
    text = _HOME.sub(lambda found: found[1] + placeholder, text)
    return _IDENTIFIER.sub(
        lambda found: found[1] + placeholder[: len(found[2])].ljust(len(found[2]), "_"),
        text,
    )


def render(value: dict[str, Any]) -> str:
    return anonymise(json.dumps(value, indent=1, ensure_ascii=False)) + "\n"
