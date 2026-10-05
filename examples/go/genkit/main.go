// The Genkit for Go suite: the shared scenarios on Genkit's generate API, tools and flows.
//
// Genkit records every action - a flow, a generate call, the model call inside it, each tool - as an
// OpenTelemetry span on the global tracer provider, with the action's input and output as JSON. The
// model is the harness's fake Gemini server through Genkit's Google AI plugin, so the captures are
// deterministic and need no credentials.
package main

import (
	"context"
	"encoding/base64"
	"fmt"

	"github.com/firebase/genkit/go/ai"
	"github.com/firebase/genkit/go/genkit"
	"github.com/firebase/genkit/go/plugins/googlegenai"
	"google.golang.org/genai"

	"github.com/sideseat/sideseat/examples/go/harness"
)

var g *genkit.Genkit

func main() {
	harness.Main(harness.Suite{
		Scenarios: map[string]func(context.Context, *harness.Run) error{
			"chat":              chat,
			"multi_turn":        multiTurn,
			"session":           sessionScenario,
			"tool_use":          toolUse,
			"error":             errorScenario,
			"streaming":         streaming,
			"structured_output": structuredOutput,
			"reasoning":         reasoning,
			"files":             files,
			"multi_agent":       multiAgent,
		},
	})
}

// setup initialises Genkit against the fake Gemini server. Genkit traces through the global
// OpenTelemetry provider the harness installed, which is its documented way to export elsewhere.
func setup(ctx context.Context, run *harness.Run) (ai.ModelRef, error) {
	if err := harness.RequireSurface(run.Model, "Genkit for Go", "fake-gemini"); err != nil {
		return ai.ModelRef{}, err
	}
	url, err := harness.FakeURL(run.Model.Surface)
	if err != nil {
		return ai.ModelRef{}, err
	}
	if g == nil {
		g = genkit.Init(ctx, genkit.WithPlugins(&googlegenai.GoogleAI{APIKey: "fake", BaseURL: url}))
		defineTools()
	}
	return googlegenai.GoogleAIModelRef(run.Model.ID, nil), nil
}

var tools = map[string]ai.ToolRef{}

func defineTools() {
	for _, name := range []string{"get_weather", "get_precipitation", "book_flight"} {
		spec := harness.Tool(name)
		tools[name] = genkit.DefineToolWithInputSchema(g, spec.Name, spec.Description, spec.Parameters,
			func(_ *ai.ToolContext, input any) (string, error) {
				args, _ := input.(map[string]any)
				// A Genkit tool's error ends the generation; the failure is returned as the result
				// instead, which is what the model reads and answers from.
				result, err := harness.Call(name, args)
				return harness.ResultText(result, err), nil
			})
	}
}

func use(names ...string) []ai.ToolRef {
	var out []ai.ToolRef
	for _, name := range names {
		out = append(out, tools[name])
	}
	return out
}

func generate(ctx context.Context, run *harness.Run, opts ...ai.GenerateOption) (*ai.ModelResponse, error) {
	model, err := setup(ctx, run)
	if err != nil {
		return nil, err
	}
	return genkit.Generate(ctx, g, append([]ai.GenerateOption{ai.WithModel(model), ai.WithSystem(harness.Content.System)}, opts...)...)
}

func ask(ctx context.Context, run *harness.Run, opts ...ai.GenerateOption) error {
	return run.Trace(ctx, func(ctx context.Context) error {
		response, err := generate(ctx, run, opts...)
		if err != nil {
			return err
		}
		fmt.Println(response.Text())
		return nil
	})
}

func chat(ctx context.Context, run *harness.Run) error {
	return ask(ctx, run, ai.WithPrompt(harness.Content.Chat))
}

func multiTurn(ctx context.Context, run *harness.Run) error {
	return run.Trace(ctx, func(ctx context.Context) error {
		var history []*ai.Message
		for _, question := range harness.Content.MultiTurn {
			history = append(history, ai.NewUserTextMessage(question))
			response, err := generate(ctx, run, ai.WithMessages(history...))
			if err != nil {
				return err
			}
			fmt.Println(response.Text())
			history = append(history, response.Message)
		}
		return nil
	})
}

func sessionScenario(ctx context.Context, run *harness.Run) error {
	for index, question := range harness.Content.Session {
		err := run.TraceNamed(ctx, fmt.Sprintf("session-turn-%d", index+1), func(ctx context.Context) error {
			response, err := generate(ctx, run, ai.WithPrompt(question))
			if err != nil {
				return err
			}
			fmt.Println(response.Text())
			return nil
		})
		if err != nil {
			return err
		}
	}
	return nil
}

func toolUse(ctx context.Context, run *harness.Run) error {
	if _, err := setup(ctx, run); err != nil {
		return err
	}
	return ask(ctx, run, ai.WithPrompt(harness.Content.ToolUse), ai.WithTools(use("get_weather", "get_precipitation")...))
}

func errorScenario(ctx context.Context, run *harness.Run) error {
	if _, err := setup(ctx, run); err != nil {
		return err
	}
	return ask(ctx, run, ai.WithPrompt(harness.Content.Error), ai.WithTools(use("book_flight")...))
}

func streaming(ctx context.Context, run *harness.Run) error {
	if _, err := setup(ctx, run); err != nil {
		return err
	}
	return ask(ctx, run, ai.WithPrompt(harness.Content.Streaming), ai.WithTools(use("get_weather")...),
		ai.WithStreaming(func(_ context.Context, chunk *ai.ModelResponseChunk) error {
			fmt.Print(chunk.Text())
			return nil
		}))
}

// TripPlan is a short itinerary: the schema the structured scenario's answer is constrained to.
type TripPlan struct {
	City      string   `json:"city" jsonschema:"description=The destination city"`
	Days      []string `json:"days" jsonschema:"description=One activity per day"`
	BudgetEur int      `json:"budget_eur" jsonschema:"description=Estimated total budget in euros"`
}

func structuredOutput(ctx context.Context, run *harness.Run) error {
	return ask(ctx, run, ai.WithPrompt(harness.Content.Structured), ai.WithOutputType(TripPlan{}))
}

func reasoning(ctx context.Context, run *harness.Run) error {
	return ask(ctx, run, ai.WithPrompt(harness.Content.Reasoning),
		ai.WithConfig(&genai.GenerateContentConfig{ThinkingConfig: &genai.ThinkingConfig{IncludeThoughts: true}}))
}

func files(ctx context.Context, run *harness.Run) error {
	dataURL := func(mime, name string) string {
		return "data:" + mime + ";base64," + base64.StdEncoding.EncodeToString(harness.Asset(name))
	}
	return ask(ctx, run, ai.WithMessages(ai.NewUserMessage(
		ai.NewTextPart(harness.Content.Files),
		ai.NewMediaPart("image/jpeg", dataURL("image/jpeg", "img.jpg")),
		ai.NewMediaPart("application/pdf", dataURL("application/pdf", "task.pdf")),
	)))
}

// multiAgent hands the forecast to a specialist: a tool whose body is a generate call of its own,
// Genkit's pattern for one agent delegating to another.
func multiAgent(ctx context.Context, run *harness.Run) error {
	if _, err := setup(ctx, run); err != nil {
		return err
	}
	specialist := genkit.DefineTool(g, "weather_specialist", "Ask the weather specialist, which has the forecast tools, a question.",
		func(tc *ai.ToolContext, input struct {
			Request string `json:"request" jsonschema:"description=The question for the specialist"`
		}) (string, error) {
			response, err := generate(tc.Context, run, ai.WithPrompt(input.Request), ai.WithTools(use("get_weather", "get_precipitation")...))
			if err != nil {
				return "", err
			}
			return response.Text(), nil
		})
	return ask(ctx, run,
		ai.WithSystem(harness.Content.System+" Ask weather_specialist for any forecast you need."),
		ai.WithPrompt(harness.Content.MultiAgent), ai.WithTools(specialist))
}
