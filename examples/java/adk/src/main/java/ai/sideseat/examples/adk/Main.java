package ai.sideseat.examples.adk;

import ai.sideseat.examples.harness.Content;
import ai.sideseat.examples.harness.Model;
import ai.sideseat.examples.harness.Run;
import ai.sideseat.examples.harness.Sample;
import ai.sideseat.examples.harness.Tools;
import com.anthropic.backends.Backend;
import com.anthropic.bedrock.backends.BedrockBackend;
import com.anthropic.client.AnthropicClient;
import com.anthropic.client.okhttp.AnthropicOkHttpClient;
import com.anthropic.core.http.HttpRequest;
import com.anthropic.core.http.HttpResponse;
import com.anthropic.models.messages.Message;
import com.anthropic.services.blocking.MessageService;
import com.google.adk.agents.BaseAgent;
import com.google.adk.agents.LlmAgent;
import com.google.adk.events.Event;
import com.google.adk.models.Claude;
import com.google.adk.runner.InMemoryRunner;
import com.google.adk.tools.Annotations.Schema;
import com.google.adk.tools.FunctionTool;
import com.google.genai.types.Part;
import java.lang.reflect.InvocationTargetException;
import java.lang.reflect.Method;
import java.lang.reflect.Proxy;
import java.util.ArrayList;
import java.util.List;
import java.util.Map;
import software.amazon.awssdk.auth.credentials.AwsBasicCredentials;
import software.amazon.awssdk.auth.credentials.DefaultCredentialsProvider;
import software.amazon.awssdk.auth.credentials.StaticCredentialsProvider;
import software.amazon.awssdk.regions.Region;

/**
 * The Google ADK for Java suite: the shared scenarios on ADK agents, with ADK's Claude model on the
 * Anthropic SDK's Bedrock backend.
 *
 * <p>ADK traces itself through the global OpenTelemetry instance, with the attributes the Python ADK
 * writes ({@code gcp.vertex.agent.llm_request}, {@code gcp.vertex.agent.llm_response}, the tool call
 * arguments and responses), so the documented setup is an SDK registered globally before the first
 * agent runs, which the harness's pipeline is.
 *
 * <p>ADK's Claude model sends text, tool calls and tool results only, reads no thinking, and calls the
 * model without streaming, so there are no {@code files}, {@code reasoning} or {@code streaming} scenarios.
 */
public final class Main {
  private static final List<String> PLANS = new ArrayList<>();

  /** The shared tools. ADK builds each declaration from the method's name, parameters and annotations. */
  public static final class TravelTools {
    private TravelTools() {}

    @Schema(name = "get_weather", description = "Get the weather forecast for a city.")
    public static Map<String, Object> getWeather(
        @Schema(name = "city", description = "The city name.") String city,
        @Schema(name = "days", description = "How many days to forecast, from 1 to 7.", optional = true) Integer days) {
      return Tools.getWeather(city, days == null ? 1 : days);
    }

    @Schema(name = "get_precipitation", description = "Get the chance of rain in a city for tomorrow.")
    public static String getPrecipitation(@Schema(name = "city", description = "The city name.") String city) {
      return Tools.getPrecipitation(city);
    }

    // ADK's function tool answers any exception with a fixed "An internal error occurred." and runs no
    // error callback, so the tool reports its failure as a result, the way ADK's tool guide shows.
    @Schema(name = "book_flight", description = "Book a flight.")
    public static Map<String, Object> bookFlight(
        @Schema(name = "origin", description = "Departure city.") String origin,
        @Schema(name = "destination", description = "Arrival city.") String destination,
        @Schema(name = "date", description = "Travel date.") String date) {
      try {
        return Map.of("result", Tools.bookFlight(origin, destination, date));
      } catch (Tools.ToolError e) {
        return Map.of("error", e.kind + ": " + e.getMessage());
      }
    }

    // ADK's output schema turns the model's tools off and asks for JSON, which ADK's Claude model does not
    // forward; the schema is a tool the model chooses to hand its plan to, as in the Python suite.
    @Schema(name = "trip_plan", description = "Record the finished trip plan.")
    public static String tripPlan(
        @Schema(name = "city", description = "The destination city.") String city,
        @Schema(name = "days", description = "One activity per day.") List<String> days,
        @Schema(name = "budget_eur", description = "Estimated total budget in euros.") int budgetEur) {
      PLANS.add(city + " " + days + " " + budgetEur + " EUR");
      return "Plan recorded.";
    }
  }

  private static FunctionTool tool(String method) {
    return FunctionTool.create(TravelTools.class, method);
  }

  /** The Anthropic SDK on Bedrock, or on the capture tool's recording proxy, which signs what it forwards. */
  private static AnthropicClient anthropic() {
    var bedrock = BedrockBackend.builder().region(Region.of(Model.region()));
    String proxy = Model.modelProxy();
    bedrock.awsCredentialsProvider(proxy != null
        ? StaticCredentialsProvider.create(AwsBasicCredentials.create("proxy", "proxy"))
        : DefaultCredentialsProvider.builder().build());
    Backend backend = proxy == null ? bedrock.build() : new Proxied(bedrock.build(), proxy);
    return withoutThinking(AnthropicOkHttpClient.builder().backend(backend).timeout(Model.REQUEST_TIMEOUT).maxRetries(0).build());
  }

  /** The Bedrock backend at another address: the client takes its base URL from the backend alone. */
  private record Proxied(Backend bedrock, String baseUrl) implements Backend {
    @Override
    public HttpRequest prepareRequest(HttpRequest request) {
      return bedrock.prepareRequest(request);
    }

    @Override
    public HttpRequest authorizeRequest(HttpRequest request) {
      return bedrock.authorizeRequest(request);
    }

    @Override
    public HttpResponse prepareResponse(HttpResponse response) {
      return bedrock.prepareResponse(response);
    }

    @Override
    public void close() {
      bedrock.close();
    }
  }

  /**
   * The client with thinking blocks left out of every response. Current Claude models always think,
   * and ADK's Claude model converts only text and tool-use blocks back, failing on any other. ADK
   * records its own request and response, which this does not change: ADK never sees the thinking.
   */
  private static AnthropicClient withoutThinking(AnthropicClient client) {
    MessageService messages = client.messages();
    MessageService patched = (MessageService) Proxy.newProxyInstance(
        MessageService.class.getClassLoader(), new Class<?>[] {MessageService.class},
        (proxy, method, arguments) -> {
          Object result = invoke(method, messages, arguments);
          if (method.getName().equals("create") && result instanceof Message message) {
            return message.toBuilder()
                .content(message.content().stream().filter(b -> b.isText() || b.isToolUse()).toList())
                .build();
          }
          return result;
        });
    return (AnthropicClient) Proxy.newProxyInstance(
        AnthropicClient.class.getClassLoader(), new Class<?>[] {AnthropicClient.class},
        (proxy, method, arguments) -> method.getName().equals("messages") && method.getParameterCount() == 0
            ? patched
            : invoke(method, client, arguments));
  }

  private static Object invoke(Method method, Object target, Object[] arguments) throws Throwable {
    try {
      return method.invoke(target, arguments);
    } catch (InvocationTargetException e) {
      throw e.getCause();
    }
  }

  private static Claude claude(Run run) {
    run.model.require("ADK", "bedrock-anthropic");
    return new Claude(run.model.id(), anthropic());
  }

  private static LlmAgent.Builder agent(Run run, String name) {
    return LlmAgent.builder().name(name).model(claude(run)).instruction(Content.SYSTEM);
  }

  /** One ADK session; every {@link #ask} continues it, so each request re-sends the history. */
  private static final class Conversation {
    private final Run run;
    private final InMemoryRunner runner;
    private String sessionId;

    Conversation(Run run, BaseAgent agent) {
      this.run = run;
      this.runner = new InMemoryRunner(agent, run.producer);
    }

    String ask(String question) {
      if (sessionId == null) {
        // The ADK session takes the scenario's session id, so the session ADK records and the caller's agree.
        sessionId = runner.sessionService()
            .createSession(run.producer, Content.USER_ID, (Map<String, Object>) null, run.sessionId())
            .blockingGet()
            .id();
      }
      StringBuilder answer = new StringBuilder();
      for (Event event : runner.runAsync(Content.USER_ID, sessionId,
          com.google.genai.types.Content.fromParts(Part.fromText(question))).blockingIterable()) {
        if (event.partial().orElse(false) || event.content().isEmpty()) continue;
        for (Part part : event.content().get().parts().orElse(List.of())) {
          part.text().ifPresent(answer::append);
        }
      }
      return answer.toString();
    }
  }

  private static void ask(Run run, BaseAgent agent, String question) throws Exception {
    System.out.println(run.trace(() -> new Conversation(run, agent).ask(question)));
  }

  public static void main(String[] args) {
    new Sample()
        .scenario("chat", run -> ask(run, agent(run, "assistant").build(), Content.CHAT))
        .scenario("multi_turn", run -> {
          var conversation = new Conversation(run, agent(run, "assistant").build());
          run.trace(() -> {
            for (String question : Content.MULTI_TURN) System.out.println(conversation.ask(question));
            return null;
          });
        })
        .scenario("session", run -> {
          // Two traces in one ADK session, attributed to one session and user. An ADK session keeps its
          // history, so the second request re-sends the first trace's turn.
          var conversation = new Conversation(run, agent(run, "assistant").build());
          for (int i = 0; i < Content.SESSION.size(); i++) {
            String question = Content.SESSION.get(i);
            System.out.println(run.trace("session-turn-" + (i + 1), () -> conversation.ask(question)));
          }
        })
        .scenario("tool_use", run -> ask(run,
            agent(run, "assistant").tools(tool("getWeather"), tool("getPrecipitation")).build(), Content.TOOL_USE))
        .scenario("error", run -> ask(run, agent(run, "assistant").tools(tool("bookFlight")).build(), Content.ERROR))
        .scenario("structured_output", run -> {
          ask(run, agent(run, "planner")
              .instruction(Content.SYSTEM + " Hand the finished plan to trip_plan.")
              .tools(tool("tripPlan"))
              .build(), Content.STRUCTURED);
          System.out.println(PLANS.getLast());
        })
        .scenario("multi_agent", run -> {
          // ADK's agent transfer: the researcher hands the conversation to its writer sub-agent.
          LlmAgent writer = LlmAgent.builder().name("writer").model(claude(run))
              .description("Writes the final packing list from the researcher's findings.")
              .instruction("You write the final packing list from the researcher's findings.")
              .build();
          LlmAgent researcher = LlmAgent.builder().name("researcher").model(claude(run))
              .instruction("You research weather with the tool, then hand off to the writer.")
              .tools(tool("getWeather"))
              .subAgents(writer)
              .build();
          ask(run, researcher, Content.MULTI_AGENT);
        })
        .main(args);
  }
}
