// Package harness runs the shared scenarios for the Go example suites.
//
// The prompts, tools, scenario catalog and model aliases come from content.json, which the Python
// harness renders (examples/python/harness, `capture --export-content`), so every language holds the
// same conversations. The tool bodies are re-implemented here and checked against the results the
// Python tools produced before any scenario runs.
package harness

import (
	_ "embed"
	"encoding/json"
	"fmt"
	"math"
	"reflect"
	"strings"
)

//go:embed content.json
var contentJSON []byte

// Prompts are the shared conversation texts.
type Prompts struct {
	System     string   `json:"system"`
	Chat       string   `json:"chat"`
	MultiTurn  []string `json:"multi_turn"`
	ToolUse    string   `json:"tool_use"`
	Session    []string `json:"session"`
	Error      string   `json:"error"`
	Streaming  string   `json:"streaming"`
	Structured string   `json:"structured"`
	Reasoning  string   `json:"reasoning"`
	Files      string   `json:"files"`
	MultiAgent string   `json:"multi_agent"`
	MCP        string   `json:"mcp"`
}

// ToolSpec is a shared tool's name, description and JSON schema.
type ToolSpec struct {
	Name        string         `json:"name"`
	Description string         `json:"description"`
	Parameters  map[string]any `json:"parameters"`
}

// ScenarioSpec is one entry of the shared scenario catalog.
type ScenarioSpec struct {
	Name    string `json:"name"`
	Summary string `json:"summary"`
	Core    bool   `json:"core"`
}

// ModelEntry is one model alias.
type ModelEntry struct {
	Surface   string `json:"surface"`
	ID        string `json:"id"`
	Reasoning bool   `json:"reasoning"`
}

type toolExample struct {
	Arguments map[string]any `json:"arguments"`
	Result    any            `json:"result"`
	Error     *struct {
		Name    string `json:"name"`
		Message string `json:"message"`
	} `json:"error"`
}

type document struct {
	Prompts      Prompts                  `json:"prompts"`
	TripPlan     map[string]any           `json:"trip_plan"`
	Tools        []ToolSpec               `json:"tools"`
	ToolExamples map[string][]toolExample `json:"tool_examples"`
	Scenarios    []ScenarioSpec           `json:"scenarios"`
	UserID       string                   `json:"user_id"`
	Models       map[string]ModelEntry    `json:"models"`
	DefaultModel string                   `json:"default_model"`
}

var doc = func() document {
	var d document
	if err := json.Unmarshal(contentJSON, &d); err != nil {
		panic(fmt.Sprintf("content.json: %v", err))
	}
	return d
}()

// Content is the shared prompts.
var Content = doc.Prompts

// TripPlanSchema is the JSON schema of a trip plan: city, one activity per day, a budget in euros.
var TripPlanSchema = doc.TripPlan

// UserID is the user every scenario is attributed to.
var UserID = doc.UserID

// Tool returns a shared tool's definition.
func Tool(name string) ToolSpec {
	for _, t := range doc.Tools {
		if t.Name == name {
			return t
		}
	}
	panic("no shared tool " + name)
}

// ToolError is a failure a tool reports to the model, named like the Python exception.
type ToolError struct{ Kind, Message string }

func (e *ToolError) Error() string { return e.Message }

var rainy = map[string]bool{"tokyo": true, "london": true, "oslo": true, "barcelona": true}

// Forecast is one day of get_weather.
type Forecast struct {
	Day       int    `json:"day"`
	Condition string `json:"condition"`
	HighC     int    `json:"high_c"`
}

// Weather is get_weather's result.
type Weather struct {
	City     string     `json:"city"`
	Forecast []Forecast `json:"forecast"`
}

// GetWeather is the shared get_weather tool.
func GetWeather(city string, days int) Weather {
	wet := rainy[strings.ToLower(strings.TrimSpace(city))]
	count := int(math.Max(1, math.Min(float64(days), 7)))
	w := Weather{City: city}
	for day := range count {
		condition := "sunny"
		if wet && day == 0 {
			condition = "rain"
		}
		w.Forecast = append(w.Forecast, Forecast{Day: day + 1, Condition: condition, HighC: 21 + day})
	}
	return w
}

// GetPrecipitation is the shared get_precipitation tool.
func GetPrecipitation(city string) string {
	chance := 10
	if rainy[strings.ToLower(strings.TrimSpace(city))] {
		chance = 80
	}
	return fmt.Sprintf("%d%% chance of rain in %s tomorrow.", chance, city)
}

// BookFlight is the shared book_flight tool, which always fails.
func BookFlight(origin, destination, date string) (string, error) {
	return "", &ToolError{
		Kind:    "BookingUnavailable",
		Message: fmt.Sprintf("No seats from %s to %s on %s: the booking system is offline.", origin, destination, date),
	}
}

// Call runs a shared tool by name with JSON-decoded arguments.
func Call(name string, args map[string]any) (any, error) {
	str := func(key string) string { s, _ := args[key].(string); return s }
	switch name {
	case "get_weather":
		// Decoders disagree on what a JSON number becomes, so every numeric form is accepted.
		days := 1
		switch v := args["days"].(type) {
		case float64:
			days = int(v)
		case int:
			days = v
		case int64:
			days = int(v)
		case json.Number:
			if n, err := v.Int64(); err == nil {
				days = int(n)
			}
		}
		return GetWeather(str("city"), days), nil
	case "get_precipitation":
		return GetPrecipitation(str("city")), nil
	case "book_flight":
		return BookFlight(str("origin"), str("destination"), str("date"))
	}
	return nil, fmt.Errorf("unknown tool %s", name)
}

// ResultText is what a model reads for a tool's outcome: JSON for structured values, and a failure
// as `Kind: message`, the way the Python harness reports one.
func ResultText(value any, err error) string {
	if err != nil {
		if te, ok := err.(*ToolError); ok {
			return te.Kind + ": " + te.Message
		}
		return err.Error()
	}
	if s, ok := value.(string); ok {
		return s
	}
	b, _ := json.Marshal(value)
	return string(b)
}

func init() {
	for name, examples := range doc.ToolExamples {
		for _, ex := range examples {
			got, err := Call(name, ex.Arguments)
			var outcome, want any
			if err != nil {
				te := err.(*ToolError)
				outcome = map[string]any{"name": te.Kind, "message": te.Message}
				if ex.Error != nil {
					want = map[string]any{"name": ex.Error.Name, "message": ex.Error.Message}
				}
			} else {
				b, _ := json.Marshal(got)
				_ = json.Unmarshal(b, &outcome)
				want = ex.Result
			}
			if !reflect.DeepEqual(outcome, want) {
				panic(fmt.Sprintf("%s(%v) returned %v; the Python tool returns %v", name, ex.Arguments, outcome, want))
			}
		}
	}
}
