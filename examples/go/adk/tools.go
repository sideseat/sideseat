package main

import (
	"encoding/json"
	"fmt"

	"github.com/google/jsonschema-go/jsonschema"
	"google.golang.org/adk/tool"
	"google.golang.org/adk/tool/functiontool"

	"github.com/sideseat/sideseat/examples/go/harness"
)

// sharedTool is one of the harness's tools as an ADK function tool, with the shared definition.
func sharedTool(name string) (tool.Tool, error) {
	spec := harness.Tool(name)
	raw, _ := json.Marshal(spec.Parameters)
	var schema jsonschema.Schema
	if err := json.Unmarshal(raw, &schema); err != nil {
		return nil, fmt.Errorf("%s schema: %w", name, err)
	}
	return functiontool.New(
		functiontool.Config{Name: spec.Name, Description: spec.Description, InputSchema: &schema},
		func(_ tool.Context, args map[string]any) (any, error) {
			result, err := harness.Call(name, args)
			if err != nil {
				// The model reads the failure and answers anyway.
				return nil, fmt.Errorf("%s", harness.ResultText(nil, err))
			}
			return result, nil
		},
	)
}

func sharedTools(names ...string) ([]tool.Tool, error) {
	var tools []tool.Tool
	for _, name := range names {
		t, err := sharedTool(name)
		if err != nil {
			return nil, err
		}
		tools = append(tools, t)
	}
	return tools, nil
}
