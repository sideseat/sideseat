package harness

import (
	"fmt"
	"os"
	"sort"
	"time"
)

// Model is one alias of the shared model catalog.
type Model struct {
	Alias string
	ModelEntry
}

// RequestTimeout is how long one model request may take. A client that times out and retries
// records a second, different answer.
const RequestTimeout = 600 * time.Second

// Resolve finds a model alias.
func Resolve(alias string) (Model, error) {
	if entry, ok := doc.Models[alias]; ok {
		return Model{Alias: alias, ModelEntry: entry}, nil
	}
	return Model{}, usageError(fmt.Sprintf("unknown model %q; choose one of: %v", alias, aliases()))
}

func aliases() []string {
	var names []string
	for alias := range doc.Models {
		names = append(names, alias)
	}
	sort.Strings(names)
	return names
}

// Region is the AWS region the Bedrock clients use.
func Region() string {
	for _, key := range []string{"AWS_REGION", "AWS_DEFAULT_REGION"} {
		if v := os.Getenv(key); v != "" {
			return v
		}
	}
	return "us-east-1"
}

// ModelProxy is the capture tool's recording proxy in front of bedrock-runtime, if one runs. It signs
// what it forwards with the ambient credentials and replays recorded answers without any.
func ModelProxy() string { return os.Getenv("SIDESEAT_MODEL_PROXY") }

// BedrockRuntimeURL is bedrock-runtime, or the proxy standing in front of it.
func BedrockRuntimeURL() string {
	if proxy := ModelProxy(); proxy != "" {
		return proxy
	}
	return fmt.Sprintf("https://bedrock-runtime.%s.amazonaws.com", Region())
}

// FakeURL is the deterministic fake server for a fake-* surface, started by the capture tool.
func FakeURL(surface string) (string, error) {
	key := ""
	for _, r := range surface {
		switch {
		case r == '-':
			key += "_"
		case r >= 'a' && r <= 'z':
			key += string(r - 32)
		default:
			key += string(r)
		}
	}
	if v := os.Getenv(key + "_URL"); v != "" {
		return v, nil
	}
	return "", fmt.Errorf("%s_URL is not set: run fake-model scenarios through `make capture`, which starts the fake", key)
}

// RequireSurface refuses a model the suite cannot drive.
func RequireSurface(m Model, suite string, surfaces ...string) error {
	for _, s := range surfaces {
		if m.Surface == s {
			return nil
		}
	}
	return usageError(fmt.Sprintf("the %s suite runs %v models; %s is %s", suite, surfaces, m.Alias, m.Surface))
}
