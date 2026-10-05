package ai.sideseat.examples.harness;

import java.nio.file.Files;
import java.nio.file.Path;

/** Locates the repository's {@code examples} directory: {@code SIDESEAT_EXAMPLES}, or above the suite. */
public final class Examples {
  private Examples() {}

  public static Path dir() {
    String configured = System.getenv("SIDESEAT_EXAMPLES");
    if (configured != null && !configured.isBlank()) {
      return Path.of(configured).toAbsolutePath();
    }
    for (Path dir = Path.of("").toAbsolutePath(); dir != null; dir = dir.getParent()) {
      if (Files.isRegularFile(dir.resolve("assets/img.jpg"))) {
        return dir;
      }
    }
    throw new IllegalStateException("examples/assets was not found above the working directory");
  }

  /** An input file from {@code examples/assets}: {@code img.jpg} or {@code task.pdf}. */
  public static byte[] asset(String name) {
    try {
      return Files.readAllBytes(dir().resolve("assets").resolve(name));
    } catch (java.io.IOException e) {
      throw new java.io.UncheckedIOException(e);
    }
  }

  /** The stdio command that starts the example MCP calculator server. */
  public static java.util.List<String> mcpCalculator() {
    Path server = dir().getParent().resolve("scripts/tools/mcp-calculator");
    return java.util.List.of("uv", "run", "--locked", "--directory", server.toString(), "mcp-calculator");
  }
}
