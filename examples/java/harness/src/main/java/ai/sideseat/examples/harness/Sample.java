package ai.sideseat.examples.harness;

import com.fasterxml.jackson.databind.JsonNode;
import java.nio.file.Files;
import java.nio.file.Path;
import java.util.ArrayList;
import java.util.LinkedHashMap;
import java.util.List;
import java.util.Map;
import java.util.function.Consumer;

/**
 * {@code sample}: the command every JVM suite runs, with the Python harness's command line. Run it
 * from the repository root, naming the suite (here Koog) with {@code -p}.
 *
 * <pre>
 * examples/java/gradlew -p examples/java/koog -q run --args="--list"
 * examples/java/gradlew -p examples/java/koog -q run --args="tool_use"
 * examples/java/gradlew -p examples/java/koog -q run --args="tool_use --sideseat --model haiku"
 * </pre>
 *
 * The suite is the working directory: it holds {@code suite.json} ({@code producer}, {@code
 * integrations}, optional {@code default-model} and {@code service-name}).
 */
public final class Sample {
  /** A scenario body, which may throw. */
  @FunctionalInterface
  public interface Body<T> {
    T run() throws Exception;
  }

  /** A scenario. */
  @FunctionalInterface
  public interface Scenario {
    void run(Run run) throws Exception;
  }

  /** A mistake in the command line, reported without a stack trace. */
  public static final class UsageError extends RuntimeException {
    public UsageError(String message) {
      super(message);
    }
  }

  private final Map<String, Scenario> scenarios = new LinkedHashMap<>();
  private Consumer<Telemetry> configure = t -> {};

  /** The framework's own telemetry setup, installed on the pipeline as its documentation says. */
  public Sample configure(Consumer<Telemetry> setup) {
    this.configure = setup;
    return this;
  }

  public Sample scenario(String name, Scenario body) {
    scenarios.put(name, body);
    return this;
  }

  public void main(String[] args) {
    try {
      run(args);
    } catch (UsageError e) {
      System.err.println(e.getMessage());
      System.exit(2);
    } catch (Exception e) {
      e.printStackTrace();
      System.exit(1);
    }
  }

  private void run(String[] args) throws Exception {
    Path manifestPath = Path.of("suite.json");
    if (!Files.isRegularFile(manifestPath)) {
      throw new UsageError("no suite.json here; run the sample from a suite directory");
    }
    JsonNode manifest = Content.JSON.readTree(Files.readString(manifestPath));
    String producer = manifest.get("producer").asText();
    String defaultModel = manifest.path("default-model").asText(Content.defaultModel());
    List<String> integrations = new ArrayList<>();
    manifest.path("integrations").forEach(n -> integrations.add(n.asText()));

    boolean sideseat = false;
    boolean list = false;
    String alias = defaultModel;
    List<String> positional = new ArrayList<>();
    for (int i = 0; i < args.length; i++) {
      switch (args[i]) {
        case "--sideseat" -> sideseat = true;
        case "--list" -> list = true;
        case "--model" -> alias = args[++i];
        default -> positional.add(args[i]);
      }
    }
    var catalog = Content.scenarios();
    for (String name : scenarios.keySet()) {
      if (catalog.stream().noneMatch(s -> s.name().equals(name))) {
        throw new UsageError("scenario outside the catalog: " + name);
      }
    }
    var available = catalog.stream().filter(s -> scenarios.containsKey(s.name())).toList();
    if (list || positional.isEmpty()) {
      System.out.println("Scenarios:");
      available.forEach(s -> System.out.printf("  %-18s %s%n", s.name(), s.summary()));
      System.out.println("\nModels:");
      for (String a : Model.aliases()) {
        Model m = Model.resolve(a);
        System.out.printf("  %-18s %s: %s%s%n", a, m.surface(), m.id(), a.equals(defaultModel) ? " (default)" : "");
      }
      return;
    }
    List<String> selected = positional.size() == 1 && positional.getFirst().equals("all")
        ? available.stream().map(Content.Scenario::name).toList()
        : positional;
    for (String name : selected) {
      if (!scenarios.containsKey(name)) throw new UsageError(producer + " has no scenario " + name);
    }
    Tools.check();
    Model model = Model.resolve(alias);
    String mode = sideseat ? "sdk" : "native";
    Telemetry telemetry = new Telemetry(mode, manifest.path("service-name").asText(producer), integrations);
    configure.accept(telemetry);
    List<String> failures = new ArrayList<>();
    for (String name : selected) {
      System.out.printf("%n=== %s / %s (%s, %s) ===%n", producer, name, mode, model.alias());
      long started = System.nanoTime();
      try {
        scenarios.get(name).run(new Run(producer, name, model, telemetry));
      } catch (Exception e) {
        e.printStackTrace();
        failures.add(name + ": " + e);
        continue;
      }
      System.out.printf("--- %s finished in %.1fs%n", name, (System.nanoTime() - started) / 1e9);
    }
    telemetry.shutdown();
    if (!failures.isEmpty()) {
      throw new IllegalStateException(String.join("\n", failures));
    }
  }
}
