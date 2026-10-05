// The Google ADK for Go suite: the shared scenarios on ADK agents, runners and tools.
//
// ADK's own telemetry records each invocation, model call and tool execution as OpenTelemetry
// spans, and the conversation itself as OpenTelemetry GenAI log events on the same pipeline. The
// model is the harness's fake Gemini server: ADK for Go ships a Gemini client only, and the fake is
// deterministic, so the captures need no credentials.
package main

import (
	"context"
	"fmt"
	"iter"

	"google.golang.org/adk/agent"
	"google.golang.org/adk/agent/llmagent"
	"google.golang.org/adk/model"
	"google.golang.org/adk/model/gemini"
	"google.golang.org/adk/runner"
	"google.golang.org/adk/session"
	"google.golang.org/adk/telemetry"
	"google.golang.org/genai"

	"github.com/sideseat/sideseat/examples/go/harness"
)

func main() {
	harness.Main(harness.Suite{
		Configure: configure,
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
			"mcp_tools":         mcpTools,
		},
	})
}

// configure is ADK's documented setup on an application's own providers: ADK records on the
// tracer and logger providers it is given, with message content captured.
func configure(t *harness.Telemetry) error {
	providers, err := telemetry.New(context.Background(),
		telemetry.WithTracerProvider(t.Tracer),
		telemetry.WithLoggerProvider(t.Logger),
		telemetry.WithGenAICaptureMessageContent(true),
	)
	if err != nil {
		return err
	}
	providers.SetGlobalOtelProviders()
	return nil
}

func newModel(ctx context.Context, run *harness.Run) (model.LLM, error) {
	if err := harness.RequireSurface(run.Model, "ADK for Go", "fake-gemini"); err != nil {
		return nil, err
	}
	url, err := harness.FakeURL(run.Model.Surface)
	if err != nil {
		return nil, err
	}
	return gemini.NewModel(ctx, run.Model.ID, &genai.ClientConfig{
		APIKey:      "fake",
		Backend:     genai.BackendGeminiAPI,
		HTTPOptions: genai.HTTPOptions{BaseURL: url},
	})
}

// conversation is one ADK session the scenario talks to.
type conversation struct {
	run    *runner.Runner
	userID string
	id     string
	config agent.RunConfig
}

func newConversation(run *harness.Run, a agent.Agent) (*conversation, error) {
	r, err := runner.New(runner.Config{
		AppName:           run.Producer,
		Agent:             a,
		SessionService:    session.InMemoryService(),
		AutoCreateSession: true,
	})
	if err != nil {
		return nil, err
	}
	return &conversation{run: r, userID: harness.UserID, id: run.SessionID()}, nil
}

// ask sends one user turn and returns the final text the agent answered with.
func (c *conversation) ask(ctx context.Context, parts ...*genai.Part) (string, error) {
	return finalText(c.run.Run(ctx, c.userID, c.id, genai.NewContentFromParts(parts, genai.RoleUser), c.config))
}

func finalText(events iter.Seq2[*session.Event, error]) (string, error) {
	answer := ""
	for event, err := range events {
		if err != nil {
			return "", err
		}
		if event.Content == nil || event.Partial || !event.IsFinalResponse() {
			continue
		}
		for _, part := range event.Content.Parts {
			if part.Text != "" && !part.Thought {
				answer += part.Text
			}
		}
	}
	return answer, nil
}

func text(s string) *genai.Part { return genai.NewPartFromText(s) }

// single runs one question against an agent built from cfg, inside the scenario's trace.
func single(ctx context.Context, run *harness.Run, cfg llmagent.Config, parts ...*genai.Part) error {
	m, err := newModel(ctx, run)
	if err != nil {
		return err
	}
	cfg.Model = m
	if cfg.Name == "" {
		cfg.Name = "assistant"
	}
	if cfg.Instruction == "" {
		cfg.Instruction = harness.Content.System
	}
	a, err := llmagent.New(cfg)
	if err != nil {
		return err
	}
	c, err := newConversation(run, a)
	if err != nil {
		return err
	}
	return run.Trace(ctx, func(ctx context.Context) error {
		answer, err := c.ask(ctx, parts...)
		fmt.Println(answer)
		return err
	})
}

func chat(ctx context.Context, run *harness.Run) error {
	return single(ctx, run, llmagent.Config{}, text(harness.Content.Chat))
}

func assistant(ctx context.Context, run *harness.Run) (*conversation, error) {
	m, err := newModel(ctx, run)
	if err != nil {
		return nil, err
	}
	a, err := llmagent.New(llmagent.Config{Name: "assistant", Model: m, Instruction: harness.Content.System})
	if err != nil {
		return nil, err
	}
	return newConversation(run, a)
}

func multiTurn(ctx context.Context, run *harness.Run) error {
	c, err := assistant(ctx, run)
	if err != nil {
		return err
	}
	return run.Trace(ctx, func(ctx context.Context) error {
		for _, question := range harness.Content.MultiTurn {
			answer, err := c.ask(ctx, text(question))
			if err != nil {
				return err
			}
			fmt.Println(answer)
		}
		return nil
	})
}

// sessionScenario holds two traces in one ADK session, so the second request re-sends the first
// trace's turn, attributed to one session and user.
func sessionScenario(ctx context.Context, run *harness.Run) error {
	c, err := assistant(ctx, run)
	if err != nil {
		return err
	}
	for index, question := range harness.Content.Session {
		err := run.TraceNamed(ctx, fmt.Sprintf("session-turn-%d", index+1), func(ctx context.Context) error {
			answer, err := c.ask(ctx, text(question))
			fmt.Println(answer)
			return err
		})
		if err != nil {
			return err
		}
	}
	return nil
}
