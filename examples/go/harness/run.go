package harness

import (
	"context"
	"os"
	"path/filepath"
	"strings"
)

// Run is what a scenario receives: the model, the telemetry, and correlation identifiers.
type Run struct {
	Producer  string
	Scenario  string
	Model     Model
	Telemetry *Telemetry
}

// SessionID is deterministic, so a recapture produces comparable fixtures.
func (r *Run) SessionID() string { return r.Producer + "-" + r.Scenario }

// Trace runs fn under a root span named for the scenario.
func (r *Run) Trace(ctx context.Context, fn func(context.Context) error) error {
	return r.TraceNamed(ctx, strings.ReplaceAll(r.Scenario, "_", "-"), fn)
}

// TraceNamed runs fn under a root span with the given name.
func (r *Run) TraceNamed(ctx context.Context, name string, fn func(context.Context) error) error {
	return r.Telemetry.trace(ctx, name, r.SessionID(), UserID, fn)
}

// Asset reads an input file from examples/assets: img.jpg or task.pdf.
func Asset(name string) []byte {
	data, err := os.ReadFile(filepath.Join(examplesDir(), "assets", name))
	if err != nil {
		panic(err)
	}
	return data
}

// examplesDir is the repository's examples directory: SIDESEAT_EXAMPLES, or the one above the suite.
func examplesDir() string {
	if dir := os.Getenv("SIDESEAT_EXAMPLES"); dir != "" {
		return dir
	}
	dir, _ := os.Getwd()
	for {
		if info, err := os.Stat(filepath.Join(dir, "assets", "img.jpg")); err == nil && !info.IsDir() {
			return dir
		}
		parent := filepath.Dir(dir)
		if parent == dir {
			panic("examples/assets was not found above " + dir)
		}
		dir = parent
	}
}

// MCPCalculator is the stdio command that starts the example MCP calculator server.
func MCPCalculator() (string, []string) {
	dir := filepath.Join(filepath.Dir(examplesDir()), "scripts", "tools", "mcp-calculator")
	return "uv", []string{"run", "--locked", "--directory", dir, "mcp-calculator"}
}
