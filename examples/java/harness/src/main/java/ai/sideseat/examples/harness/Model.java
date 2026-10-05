package ai.sideseat.examples.harness;

import com.fasterxml.jackson.databind.JsonNode;
import java.time.Duration;
import java.util.ArrayList;
import java.util.List;

/** One alias of the shared model catalog. */
public record Model(String alias, String surface, String id, boolean reasoning) {
  /**
   * How long one model request may take. A summarised reasoning request can run for minutes, and a
   * client that times out and retries records a second, different answer.
   */
  public static final Duration REQUEST_TIMEOUT = Duration.ofSeconds(600);

  static Model resolve(String alias) {
    JsonNode entry = Content.models().get(alias);
    if (entry == null) {
      throw new Sample.UsageError("unknown model \"" + alias + "\"; choose one of: " + aliases());
    }
    return new Model(alias, entry.get("surface").asText(), entry.get("id").asText(), entry.get("reasoning").asBoolean());
  }

  static List<String> aliases() {
    List<String> names = new ArrayList<>();
    Content.models().fieldNames().forEachRemaining(names::add);
    return names;
  }

  /** The AWS region the Bedrock clients use. */
  public static String region() {
    for (String key : List.of("AWS_REGION", "AWS_DEFAULT_REGION")) {
      String v = System.getenv(key);
      if (v != null && !v.isBlank()) return v;
    }
    return "us-east-1";
  }

  /**
   * The capture tool's recording proxy in front of bedrock-runtime, or null. It signs what it forwards
   * with the ambient credentials and replays recorded answers without any.
   */
  public static String modelProxy() {
    String proxy = System.getenv("SIDESEAT_MODEL_PROXY");
    return proxy == null || proxy.isBlank() ? null : proxy;
  }

  /** The deterministic fake server for a {@code fake-*} surface, started by the capture tool. */
  public static String fakeUrl(String surface) {
    String key = surface.toUpperCase().replace('-', '_') + "_URL";
    String url = System.getenv(key);
    if (url == null || url.isBlank()) {
      throw new IllegalStateException(key + " is not set: run fake-model scenarios through `make capture`, which starts the fake");
    }
    return url;
  }

  /** Refuses a model the suite cannot drive. */
  public Model require(String suite, String... surfaces) {
    for (String s : surfaces) if (s.equals(surface)) return this;
    throw new Sample.UsageError("the " + suite + " suite runs " + String.join(" or ", surfaces) + " models; " + alias + " is " + surface);
  }
}
