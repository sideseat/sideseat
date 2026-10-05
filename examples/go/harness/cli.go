package harness

import (
	"context"
	"encoding/json"
	"errors"
	"flag"
	"fmt"
	"os"
	"strings"
	"time"
)

// Manifest is a suite's suite.json.
type Manifest struct {
	Producer     string   `json:"producer"`
	Integrations []string `json:"integrations"`
	DefaultModel string   `json:"default-model"`
	ServiceName  string   `json:"service-name"`
}

// Suite is what a suite's main package hands to Main.
type Suite struct {
	// Configure installs the framework's own telemetry on the pipeline, as its documentation says.
	Configure func(t *Telemetry) error
	// Scenarios by catalog name.
	Scenarios map[string]func(ctx context.Context, run *Run) error
}

type usageError string

func (e usageError) Error() string { return string(e) }

// Main runs `sample`, with the Python harness's command line:
//
//	go run . --list
//	go run . tool_use
//	go run . tool_use --sideseat --model haiku
func Main(s Suite) {
	if err := run(s, os.Args[1:]); err != nil {
		fmt.Fprintln(os.Stderr, err)
		if errors.As(err, new(usageError)) {
			os.Exit(2)
		}
		os.Exit(1)
	}
}

func run(s Suite, args []string) error {
	raw, err := os.ReadFile("suite.json")
	if err != nil {
		return usageError("no suite.json here; run `go run .` from a suite directory")
	}
	var m Manifest
	if err := json.Unmarshal(raw, &m); err != nil {
		return err
	}
	if m.DefaultModel == "" {
		m.DefaultModel = doc.DefaultModel
	}
	// Flags may follow the scenario names, as on the other harnesses' command lines.
	fs := flag.NewFlagSet("sample", flag.ContinueOnError)
	sideseat := fs.Bool("sideseat", false, "run under SideSeat's OpenTelemetry recipe")
	alias := fs.String("model", m.DefaultModel, "model alias")
	list := fs.Bool("list", false, "list scenarios and models")
	var positional []string
	for len(args) > 0 {
		if err := fs.Parse(args); err != nil {
			return usageError(err.Error())
		}
		args = fs.Args()
		if len(args) > 0 {
			positional = append(positional, args[0])
			args = args[1:]
		}
	}
	var available []ScenarioSpec
	for _, spec := range doc.Scenarios {
		if _, ok := s.Scenarios[spec.Name]; ok {
			available = append(available, spec)
		}
	}
	for name := range s.Scenarios {
		if !inCatalog(name) {
			return usageError("scenario outside the catalog: " + name)
		}
	}
	if *list || len(positional) == 0 {
		fmt.Println("Scenarios:")
		for _, spec := range available {
			fmt.Printf("  %-18s %s\n", spec.Name, spec.Summary)
		}
		fmt.Println("\nModels:")
		for _, a := range aliases() {
			e := doc.Models[a]
			marker := ""
			if a == m.DefaultModel {
				marker = " (default)"
			}
			fmt.Printf("  %-18s %s: %s%s\n", a, e.Surface, e.ID, marker)
		}
		return nil
	}
	selected := positional
	if len(positional) == 1 && positional[0] == "all" {
		selected = nil
		for _, spec := range available {
			selected = append(selected, spec.Name)
		}
	}
	for _, name := range selected {
		if _, ok := s.Scenarios[name]; !ok {
			return usageError(fmt.Sprintf("%s has no scenario %s", m.Producer, name))
		}
	}
	model, err := Resolve(*alias)
	if err != nil {
		return err
	}
	mode := "native"
	if *sideseat {
		mode = "sdk"
	}
	service := m.ServiceName
	if service == "" {
		service = m.Producer
	}
	tel, err := newTelemetry(mode, service, m.Integrations)
	if err != nil {
		return err
	}
	if s.Configure != nil {
		if err := s.Configure(tel); err != nil {
			return err
		}
	}
	var failures []string
	for _, name := range selected {
		fmt.Printf("\n=== %s / %s (%s, %s) ===\n", m.Producer, name, mode, model.Alias)
		started := time.Now()
		r := &Run{Producer: m.Producer, Scenario: name, Model: model, Telemetry: tel}
		if err := s.Scenarios[name](context.Background(), r); err != nil {
			fmt.Fprintln(os.Stderr, err)
			failures = append(failures, name+": "+err.Error())
			continue
		}
		fmt.Printf("--- %s finished in %.1fs\n", name, time.Since(started).Seconds())
	}
	if err := tel.shutdown(); err != nil {
		return err
	}
	if len(failures) > 0 {
		return errors.New(strings.Join(failures, "\n"))
	}
	return nil
}

func inCatalog(name string) bool {
	for _, spec := range doc.Scenarios {
		if spec.Name == name {
			return true
		}
	}
	return false
}
