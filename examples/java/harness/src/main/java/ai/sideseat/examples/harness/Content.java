package ai.sideseat.examples.harness;

import com.fasterxml.jackson.databind.JsonNode;
import com.fasterxml.jackson.databind.ObjectMapper;
import java.io.IOException;
import java.io.UncheckedIOException;
import java.nio.file.Files;
import java.util.ArrayList;
import java.util.List;
import java.util.Map;

/**
 * The prompts, tools, scenario catalog and model aliases every suite uses, read from {@code
 * content.json}, which the Python harness renders ({@code capture --export-content}) so every
 * language holds the same conversations.
 */
public final class Content {
  static final ObjectMapper JSON = new ObjectMapper();
  private static final JsonNode DOC = load();

  public static final String SYSTEM = prompt("system");
  public static final String CHAT = prompt("chat");
  public static final List<String> MULTI_TURN = prompts("multi_turn");
  public static final String TOOL_USE = prompt("tool_use");
  public static final List<String> SESSION = prompts("session");
  public static final String ERROR = prompt("error");
  public static final String STREAMING = prompt("streaming");
  public static final String STRUCTURED = prompt("structured");
  public static final String REASONING = prompt("reasoning");
  public static final String FILES = prompt("files");
  public static final String MULTI_AGENT = prompt("multi_agent");
  public static final String MCP = prompt("mcp");
  public static final String USER_ID = DOC.get("user_id").asText();

  private Content() {}

  private static JsonNode load() {
    try {
      // Beside this harness in the Gradle build, whose suites run from their own directories.
      return JSON.readTree(Files.readString(java.nio.file.Path.of("").toAbsolutePath().getParent().resolve("harness/content.json")));
    } catch (IOException e) {
      throw new UncheckedIOException("content.json: run capture --export-content", e);
    }
  }

  private static String prompt(String name) {
    return DOC.get("prompts").get(name).asText();
  }

  private static List<String> prompts(String name) {
    List<String> out = new ArrayList<>();
    DOC.get("prompts").get(name).forEach(n -> out.add(n.asText()));
    return List.copyOf(out);
  }

  /** The JSON schema of a trip plan: city, one activity per day, a budget in euros. */
  public static JsonNode tripPlanSchema() {
    return DOC.get("trip_plan");
  }

  /** A shared tool's name, description and JSON schema. */
  public record ToolSpec(String name, String description, JsonNode parameters) {}

  public static ToolSpec tool(String name) {
    for (JsonNode t : DOC.get("tools")) {
      if (t.get("name").asText().equals(name)) {
        return new ToolSpec(name, t.get("description").asText(), t.get("parameters"));
      }
    }
    throw new IllegalArgumentException("no shared tool " + name);
  }

  record Scenario(String name, String summary) {}

  static List<Scenario> scenarios() {
    List<Scenario> out = new ArrayList<>();
    DOC.get("scenarios").forEach(s -> out.add(new Scenario(s.get("name").asText(), s.get("summary").asText())));
    return out;
  }

  static JsonNode models() {
    return DOC.get("models");
  }

  static String defaultModel() {
    return DOC.get("default_model").asText();
  }

  static Map<String, List<JsonNode>> toolExamples() {
    Map<String, List<JsonNode>> out = new java.util.LinkedHashMap<>();
    DOC.get("tool_examples").properties().forEach(e -> {
      List<JsonNode> list = new ArrayList<>();
      e.getValue().forEach(list::add);
      out.put(e.getKey(), list);
    });
    return out;
  }
}
