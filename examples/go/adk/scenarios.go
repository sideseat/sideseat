package main

import (
	"context"
	"fmt"
	"os/exec"

	"github.com/modelcontextprotocol/go-sdk/mcp"
	"google.golang.org/adk/agent"
	"google.golang.org/adk/agent/llmagent"
	"google.golang.org/adk/tool"
	"google.golang.org/adk/tool/agenttool"
	"google.golang.org/adk/tool/mcptoolset"
	"google.golang.org/genai"

	"github.com/sideseat/sideseat/examples/go/harness"
)

func toolUse(ctx context.Context, run *harness.Run) error {
	tools, err := sharedTools("get_weather", "get_precipitation")
	if err != nil {
		return err
	}
	return single(ctx, run, llmagent.Config{Tools: tools}, text(harness.Content.ToolUse))
}

func errorScenario(ctx context.Context, run *harness.Run) error {
	tools, err := sharedTools("book_flight")
	if err != nil {
		return err
	}
	return single(ctx, run, llmagent.Config{Tools: tools}, text(harness.Content.Error))
}

func streaming(ctx context.Context, run *harness.Run) error {
	tools, err := sharedTools("get_weather")
	if err != nil {
		return err
	}
	m, err := newModel(ctx, run)
	if err != nil {
		return err
	}
	a, err := llmagent.New(llmagent.Config{Name: "assistant", Model: m, Instruction: harness.Content.System, Tools: tools})
	if err != nil {
		return err
	}
	c, err := newConversation(run, a)
	if err != nil {
		return err
	}
	c.config = agent.RunConfig{StreamingMode: agent.StreamingModeSSE}
	return run.Trace(ctx, func(ctx context.Context) error {
		answer, err := c.ask(ctx, text(harness.Content.Streaming))
		fmt.Println(answer)
		return err
	})
}

// structuredOutput constrains the answer with ADK's output schema, which Gemini honours natively.
func structuredOutput(ctx context.Context, run *harness.Run) error {
	return single(ctx, run, llmagent.Config{
		Name: "planner",
		OutputSchema: &genai.Schema{
			Type:        genai.TypeObject,
			Description: "A short itinerary.",
			Properties: map[string]*genai.Schema{
				"city":       {Type: genai.TypeString, Description: "The destination city"},
				"days":       {Type: genai.TypeArray, Description: "One activity per day", Items: &genai.Schema{Type: genai.TypeString}},
				"budget_eur": {Type: genai.TypeInteger, Description: "Estimated total budget in euros"},
			},
			Required: []string{"city", "days", "budget_eur"},
		},
	}, text(harness.Content.Structured))
}

func reasoning(ctx context.Context, run *harness.Run) error {
	return single(ctx, run, llmagent.Config{
		GenerateContentConfig: &genai.GenerateContentConfig{
			ThinkingConfig: &genai.ThinkingConfig{IncludeThoughts: true},
		},
	}, text(harness.Content.Reasoning))
}

func files(ctx context.Context, run *harness.Run) error {
	return single(ctx, run, llmagent.Config{},
		text(harness.Content.Files),
		genai.NewPartFromBytes(harness.Asset("img.jpg"), "image/jpeg"),
		genai.NewPartFromBytes(harness.Asset("task.pdf"), "application/pdf"),
	)
}

// multiAgent is a coordinator that delegates to a weather specialist through ADK's agent tool, so the
// specialist runs as a nested invocation whose answer is the tool's result.
func multiAgent(ctx context.Context, run *harness.Run) error {
	m, err := newModel(ctx, run)
	if err != nil {
		return err
	}
	tools, err := sharedTools("get_weather", "get_precipitation")
	if err != nil {
		return err
	}
	weather, err := llmagent.New(llmagent.Config{
		Name:        "weather_specialist",
		Model:       m,
		Description: "Answers questions about the weather with the forecast tools.",
		Instruction: harness.Content.System,
		Tools:       tools,
	})
	if err != nil {
		return err
	}
	coordinator, err := llmagent.New(llmagent.Config{
		Name:        "coordinator",
		Model:       m,
		Instruction: harness.Content.System + " Ask weather_specialist for any forecast you need.",
		Tools:       []tool.Tool{agenttool.New(weather, nil)},
	})
	if err != nil {
		return err
	}
	c, err := newConversation(run, coordinator)
	if err != nil {
		return err
	}
	return run.Trace(ctx, func(ctx context.Context) error {
		answer, err := c.ask(ctx, text(harness.Content.MultiAgent))
		fmt.Println(answer)
		return err
	})
}

// mcpTools serves the calculator over stdio MCP, the example server every suite uses.
func mcpTools(ctx context.Context, run *harness.Run) error {
	command, args := harness.MCPCalculator()
	toolset, err := mcptoolset.New(mcptoolset.Config{
		Transport: &mcp.CommandTransport{Command: exec.Command(command, args...)},
	})
	if err != nil {
		return err
	}
	return single(ctx, run, llmagent.Config{Toolsets: []tool.Toolset{toolset}}, text(harness.Content.MCP))
}
