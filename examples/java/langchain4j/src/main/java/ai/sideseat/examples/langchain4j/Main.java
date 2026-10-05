package ai.sideseat.examples.langchain4j;

import ai.sideseat.examples.harness.Content;
import ai.sideseat.examples.harness.Examples;
import ai.sideseat.examples.harness.Model;
import ai.sideseat.examples.harness.Run;
import ai.sideseat.examples.harness.Sample;
import ai.sideseat.examples.harness.Telemetry;
import ai.sideseat.examples.harness.Tools;
import com.arize.instrumentation.langchain4j.LangChain4jInstrumentor;
import com.fasterxml.jackson.databind.ObjectMapper;
import dev.langchain4j.agent.tool.P;
import dev.langchain4j.agent.tool.Tool;
import dev.langchain4j.data.message.ImageContent;
import dev.langchain4j.data.message.PdfFileContent;
import dev.langchain4j.data.message.TextContent;
import dev.langchain4j.data.message.UserMessage;
import dev.langchain4j.memory.chat.MessageWindowChatMemory;
import dev.langchain4j.model.bedrock.BedrockChatModel;
import dev.langchain4j.model.bedrock.BedrockChatRequestParameters;
import dev.langchain4j.service.AiServices;
import java.net.URI;
import java.util.Base64;
import java.util.List;
import software.amazon.awssdk.auth.credentials.AwsBasicCredentials;
import software.amazon.awssdk.auth.credentials.AwsCredentialsProvider;
import software.amazon.awssdk.auth.credentials.DefaultCredentialsProvider;
import software.amazon.awssdk.auth.credentials.StaticCredentialsProvider;
import software.amazon.awssdk.regions.Region;
import software.amazon.awssdk.services.bedrockruntime.BedrockRuntimeClient;

/**
 * The LangChain4j suite: the shared scenarios on LangChain4j AI services with Bedrock Converse.
 *
 * <p>LangChain4j has no OpenTelemetry instrumentation of its own; Arize's OpenInference
 * instrumentor is the one its users install. Its AI-service listeners record each service call,
 * model call and tool execution as OpenInference spans. Its chat-model listener is for a model used
 * directly; registered beside the service listeners it records every model call a second time.
 *
 * <p>There is no streaming scenario: for a streamed call the instrumentor records the response's Java
 * {@code toString()}, which differs from run to run, as the service span's output.
 */
public final class Main {
  private static final ObjectMapper JSON = new ObjectMapper();
  private static LangChain4jInstrumentor instrumentor;

  /** The shared tools, with the shared definitions. */
  public static final class TravelTools {
    @Tool(name = "get_weather", value = "Get the weather forecast for a city.")
    public String getWeather(@P("The city name.") String city, @P(value = "How many days to forecast, from 1 to 7.", required = false) Integer days) {
      return Tools.resultText("get_weather", JSON.createObjectNode().put("city", city).put("days", days == null ? 1 : days));
    }

    @Tool(name = "get_precipitation", value = "Get the chance of rain in a city for tomorrow.")
    public String getPrecipitation(@P("The city name.") String city) {
      return Tools.resultText("get_precipitation", JSON.createObjectNode().put("city", city));
    }

    @Tool(name = "book_flight", value = "Book a flight.")
    public String bookFlight(@P("Departure city.") String origin, @P("Arrival city.") String destination, @P("Travel date.") String date) {
      var args = JSON.createObjectNode().put("origin", origin).put("destination", destination).put("date", date);
      // LangChain4j sends a tool's exception message to the model, which reads it and answers anyway.
      throw new IllegalStateException(Tools.resultText("book_flight", args));
    }
  }

  interface Assistant {
    String chat(String message);
  }

  interface FileAssistant {
    String chat(UserMessage message);
  }

  /** A short itinerary: the schema the structured scenario's answer is constrained to. */
  public record TripPlan(String city, List<String> days, int budget_eur) {}

  interface Planner {
    TripPlan plan(String request);
  }

  /** The proxy discards a client's signature, so a client pointed at it needs no credentials of its own. */
  private static AwsCredentialsProvider credentials() {
    return Model.modelProxy() != null
        ? StaticCredentialsProvider.create(AwsBasicCredentials.create("proxy", "proxy"))
        : DefaultCredentialsProvider.builder().build();
  }

  private static BedrockChatModel model(Run run, BedrockChatRequestParameters parameters) {
    run.model.require("LangChain4j", "bedrock");
    var client = BedrockRuntimeClient.builder().region(Region.of(Model.region())).credentialsProvider(credentials());
    if (Model.modelProxy() != null) client.endpointOverride(URI.create(Model.modelProxy()));
    var builder = BedrockChatModel.builder()
        .client(client.build())
        .modelId(run.model.id())
        .timeout(Model.REQUEST_TIMEOUT)
        .maxRetries(0)
        .returnThinking(true);
    if (parameters != null) builder.defaultRequestParameters(parameters);
    return builder.build();
  }

  private static BedrockChatModel model(Run run) {
    return model(run, null);
  }

  private static <T> AiServices<T> service(Class<T> type, Run run) {
    return AiServices.builder(type)
        .chatModel(model(run))
        .systemMessage(Content.SYSTEM)
        .registerListeners(instrumentor.createAiServiceListeners());
  }

  private static void ask(Run run, Assistant assistant, String question) throws Exception {
    System.out.println(run.trace(() -> assistant.chat(question)));
  }

  public static void main(String[] args) {
    new Sample()
        .configure(Main::configure)
        .scenario("chat", run -> ask(run, service(Assistant.class, run).build(), Content.CHAT))
        .scenario("multi_turn", run -> {
          Assistant assistant = service(Assistant.class, run).chatMemory(MessageWindowChatMemory.withMaxMessages(20)).build();
          run.trace(() -> {
            for (String question : Content.MULTI_TURN) System.out.println(assistant.chat(question));
            return null;
          });
        })
        .scenario("session", run -> {
          // Two traces, each its own conversation, attributed to one session and user.
          for (int i = 0; i < Content.SESSION.size(); i++) {
            String question = Content.SESSION.get(i);
            System.out.println(run.trace("session-turn-" + (i + 1), () -> service(Assistant.class, run).build().chat(question)));
          }
        })
        .scenario("tool_use", run -> ask(run, service(Assistant.class, run).tools(new TravelTools()).build(), Content.TOOL_USE))
        .scenario("error", run -> ask(run, service(Assistant.class, run).tools(new TravelTools()).build(), Content.ERROR))
        .scenario("structured_output", run -> {
          Planner planner = service(Planner.class, run).build();
          System.out.println(run.trace(() -> planner.plan(Content.STRUCTURED)));
        })
        .scenario("reasoning", run -> {
          // Current Claude models think adaptively; a summary of the thinking is what reaches the telemetry.
          var parameters = BedrockChatRequestParameters.builder()
              .additionalModelRequestField("thinking", java.util.Map.of("type", "adaptive", "display", "summarized"))
              .build();
          Assistant assistant = AiServices.builder(Assistant.class)
              .chatModel(model(run, parameters))
              .systemMessage(Content.SYSTEM)
              .registerListeners(instrumentor.createAiServiceListeners())
              .build();
          ask(run, assistant, Content.REASONING);
        })
        .scenario("files", run -> {
          FileAssistant assistant = service(FileAssistant.class, run).build();
          var message = UserMessage.from(
              TextContent.from(Content.FILES),
              ImageContent.from(Base64.getEncoder().encodeToString(Examples.asset("img.jpg")), "image/jpeg"),
              PdfFileContent.from(Base64.getEncoder().encodeToString(Examples.asset("task.pdf")), "application/pdf"));
          System.out.println(run.trace(() -> assistant.chat(message)));
        })
        .main(args);
  }

  /** Arize's documented setup on the application's tracer provider. */
  private static void configure(Telemetry telemetry) {
    instrumentor = LangChain4jInstrumentor.instrument(telemetry.sdk.getSdkTracerProvider());
  }
}
