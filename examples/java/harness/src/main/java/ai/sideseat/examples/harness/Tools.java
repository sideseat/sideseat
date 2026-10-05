package ai.sideseat.examples.harness;

import com.fasterxml.jackson.databind.JsonNode;
import java.util.ArrayList;
import java.util.LinkedHashMap;
import java.util.List;
import java.util.Map;
import java.util.Set;

/**
 * The shared tools, re-implemented and checked against the results the Python tools produced before
 * any scenario runs, so a change on either side fails first.
 */
public final class Tools {
  private Tools() {}

  /** A failure a tool reports to the model, named like the Python exception. */
  public static final class ToolError extends RuntimeException {
    public final String kind;

    public ToolError(String kind, String message) {
      super(message);
      this.kind = kind;
    }
  }

  private static final Set<String> RAINY = Set.of("tokyo", "london", "oslo", "barcelona");

  public static Map<String, Object> getWeather(String city, int days) {
    boolean wet = RAINY.contains(city.strip().toLowerCase());
    int count = Math.max(1, Math.min(days, 7));
    List<Map<String, Object>> forecast = new ArrayList<>();
    for (int day = 0; day < count; day++) {
      Map<String, Object> entry = new LinkedHashMap<>();
      entry.put("day", day + 1);
      entry.put("condition", wet && day == 0 ? "rain" : "sunny");
      entry.put("high_c", 21 + day);
      forecast.add(entry);
    }
    Map<String, Object> out = new LinkedHashMap<>();
    out.put("city", city);
    out.put("forecast", forecast);
    return out;
  }

  public static String getPrecipitation(String city) {
    int chance = RAINY.contains(city.strip().toLowerCase()) ? 80 : 10;
    return chance + "% chance of rain in " + city + " tomorrow.";
  }

  public static String bookFlight(String origin, String destination, String date) {
    throw new ToolError(
        "BookingUnavailable",
        "No seats from " + origin + " to " + destination + " on " + date + ": the booking system is offline.");
  }

  /** Runs a shared tool by name with JSON arguments. */
  public static Object call(String name, JsonNode args) {
    return switch (name) {
      case "get_weather" -> getWeather(args.path("city").asText(), args.has("days") ? args.get("days").asInt() : 1);
      case "get_precipitation" -> getPrecipitation(args.path("city").asText());
      case "book_flight" ->
          bookFlight(args.path("origin").asText(), args.path("destination").asText(), args.path("date").asText());
      default -> throw new IllegalArgumentException("unknown tool " + name);
    };
  }

  /** What a model reads for a tool's outcome: JSON for structured values, a failure as {@code Kind: message}. */
  public static String resultText(String name, JsonNode args) {
    try {
      Object value = call(name, args);
      return value instanceof String s ? s : Content.JSON.writeValueAsString(value);
    } catch (ToolError e) {
      return e.kind + ": " + e.getMessage();
    } catch (com.fasterxml.jackson.core.JsonProcessingException e) {
      throw new IllegalStateException(e);
    }
  }

  static void check() {
    Content.toolExamples().forEach((name, examples) -> {
      for (JsonNode example : examples) {
        JsonNode outcome;
        try {
          outcome = Content.JSON.createObjectNode().set("result", Content.JSON.valueToTree(call(name, example.get("arguments"))));
        } catch (ToolError e) {
          outcome = Content.JSON.createObjectNode().set("error",
              Content.JSON.createObjectNode().put("name", e.kind).put("message", e.getMessage()));
        }
        JsonNode expected = example.has("error")
            ? Content.JSON.createObjectNode().set("error", example.get("error"))
            : Content.JSON.createObjectNode().set("result", example.get("result"));
        if (!outcome.equals(expected)) {
          throw new IllegalStateException(name + "(" + example.get("arguments") + ") returned " + outcome
              + "; the Python tool returns " + expected);
        }
      }
    });
  }
}
