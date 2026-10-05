package ai.sideseat.examples.harness;

/** What a scenario receives: the model, the telemetry, and correlation identifiers. */
public final class Run {
  public final String producer;
  public final String scenario;
  public final Model model;
  public final Telemetry telemetry;

  Run(String producer, String scenario, Model model, Telemetry telemetry) {
    this.producer = producer;
    this.scenario = scenario;
    this.model = model;
    this.telemetry = telemetry;
  }

  /** Deterministic, so a recapture produces comparable fixtures. */
  public String sessionId() {
    return producer + "-" + scenario;
  }

  /** Runs {@code body} under a root span named for the scenario. */
  public <T> T trace(Sample.Body<T> body) throws Exception {
    return trace(scenario.replace('_', '-'), body);
  }

  public <T> T trace(String name, Sample.Body<T> body) throws Exception {
    return telemetry.trace(name, sessionId(), Content.USER_ID, body);
  }
}
