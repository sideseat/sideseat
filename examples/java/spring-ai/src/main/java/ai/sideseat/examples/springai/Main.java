package ai.sideseat.examples.springai;

import ai.sideseat.examples.harness.Content;
import ai.sideseat.examples.harness.Examples;
import ai.sideseat.examples.harness.Model;
import ai.sideseat.examples.harness.Run;
import ai.sideseat.examples.harness.Sample;
import ai.sideseat.examples.harness.Telemetry;
import ai.sideseat.examples.harness.Tools;
import com.arize.instrumentation.OITracer;
import com.arize.instrumentation.springAI.SpringAIInstrumentor;
import com.fasterxml.jackson.databind.ObjectMapper;
import io.micrometer.observation.ObservationRegistry;
import java.net.URI;
import java.util.List;
import org.springframework.ai.bedrock.converse.BedrockChatOptions;
import org.springframework.ai.bedrock.converse.BedrockProxyChatModel;
import org.springframework.ai.chat.client.ChatClient;
import org.springframework.ai.chat.client.advisor.MessageChatMemoryAdvisor;
import org.springframework.ai.chat.memory.ChatMemory;
import org.springframework.ai.chat.memory.MessageWindowChatMemory;
import org.springframework.ai.tool.annotation.Tool;
import org.springframework.ai.tool.annotation.ToolParam;
import org.springframework.core.io.ByteArrayResource;
import org.springframework.util.MimeType;
import software.amazon.awssdk.auth.credentials.AwsBasicCredentials;
import software.amazon.awssdk.auth.credentials.AwsCredentialsProvider;
import software.amazon.awssdk.auth.credentials.DefaultCredentialsProvider;
import software.amazon.awssdk.auth.credentials.StaticCredentialsProvider;
import software.amazon.awssdk.http.Protocol;
import software.amazon.awssdk.http.nio.netty.NettyNioAsyncHttpClient;
import software.amazon.awssdk.regions.Region;
import software.amazon.awssdk.services.bedrockruntime.BedrockRuntimeAsyncClient;
import software.amazon.awssdk.services.bedrockruntime.BedrockRuntimeClient;

/**
 * The Spring AI suite: the shared scenarios on Spring AI's {@code ChatClient} with Bedrock Converse.
 *
 * <p>Spring AI reports each model call as a Micrometer observation and keeps prompt and completion
 * text out of its own traces. Arize's OpenInference instrumentor is the observation handler its users
 * register to get the conversation: it records each call as an OpenInference LLM span with the
 * messages it sent and received.
 */
public final class Main {
  private static final ObjectMapper JSON = new ObjectMapper();
  private static ObservationRegistry observations;

  /** The shared tools, with the shared definitions. */
  public static final class TravelTools {
    @Tool(name = "get_weather", description = "Get the weather forecast for a city.")
    public String getWeather(@ToolParam(description = "The city name.") String city,
        @ToolParam(description = "How many days to forecast, from 1 to 7.", required = false) Integer days) {
      return Tools.resultText("get_weather", JSON.createObjectNode().put("city", city).put("days", days == null ? 1 : days));
    }

    @Tool(name = "get_precipitation", description = "Get the chance of rain in a city for tomorrow.")
    public String getPrecipitation(@ToolParam(description = "The city name.") String city) {
      return Tools.resultText("get_precipitation", JSON.createObjectNode().put("city", city));
    }

    @Tool(name = "book_flight", description = "Book a flight.")
    public String bookFlight(@ToolParam(description = "Departure city.") String origin,
        @ToolParam(description = "Arrival city.") String destination, @ToolParam(description = "Travel date.") String date) {
      var args = JSON.createObjectNode().put("origin", origin).put("destination", destination).put("date", date);
      // Spring AI hands a tool's exception message to the model, which reads it and answers anyway.
      throw new IllegalStateException(Tools.resultText("book_flight", args));
    }
  }

  /** A short itinerary: the schema the structured scenario's answer is constrained to. */
  public record TripPlan(String city, List<String> days, int budget_eur) {}

  /** The proxy discards a client's signature, so a client pointed at it needs no credentials of its own. */
  private static AwsCredentialsProvider credentials() {
    return Model.modelProxy() != null
        ? StaticCredentialsProvider.create(AwsBasicCredentials.create("proxy", "proxy"))
        : DefaultCredentialsProvider.builder().build();
  }

  private static ChatClient.Builder client(Run run) {
    run.model.require("Spring AI", "bedrock");
    var sync = BedrockRuntimeClient.builder().region(Region.of(Model.region())).credentialsProvider(credentials());
    var async = BedrockRuntimeAsyncClient.builder().region(Region.of(Model.region())).credentialsProvider(credentials());
    if (Model.modelProxy() != null) {
      sync.endpointOverride(URI.create(Model.modelProxy()));
      // The capture proxy speaks HTTP/1.1; the async client would otherwise negotiate HTTP/2.
      async.endpointOverride(URI.create(Model.modelProxy()))
          .httpClientBuilder(NettyNioAsyncHttpClient.builder().protocol(Protocol.HTTP1_1).readTimeout(Model.REQUEST_TIMEOUT));
    }
    var model = BedrockProxyChatModel.builder()
        .bedrockRuntimeClient(sync.build())
        .bedrockRuntimeAsyncClient(async.build())
        .options(BedrockChatOptions.builder().model(run.model.id()).maxTokens(16_000).build())
        .timeout(Model.REQUEST_TIMEOUT)
        .observationRegistry(observations)
        .build();
    return ChatClient.builder(model, observations, null, null).defaultSystem(Content.SYSTEM);
  }

  private static void ask(Run run, ChatClient client, String question, Object... tools) throws Exception {
    System.out.println(run.trace(() -> client.prompt().user(question).tools(tools).call().content()));
  }

  public static void main(String[] args) {
    new Sample()
        .configure(Main::configure)
        .scenario("chat", run -> ask(run, client(run).build(), Content.CHAT))
        .scenario("multi_turn", run -> {
          var memory = MessageChatMemoryAdvisor.builder(MessageWindowChatMemory.builder().build()).build();
          ChatClient chat = client(run).defaultAdvisors(memory).build();
          run.trace(() -> {
            for (String question : Content.MULTI_TURN) {
              System.out.println(chat.prompt().user(question)
                  .advisors(a -> a.param(ChatMemory.CONVERSATION_ID, run.sessionId())).call().content());
            }
            return null;
          });
        })
        .scenario("session", run -> {
          // Two traces, each its own conversation, attributed to one session and user.
          for (int i = 0; i < Content.SESSION.size(); i++) {
            String question = Content.SESSION.get(i);
            ChatClient chat = client(run).build();
            System.out.println(run.trace("session-turn-" + (i + 1), () -> chat.prompt().user(question).call().content()));
          }
        })
        .scenario("tool_use", run -> ask(run, client(run).build(), Content.TOOL_USE, new TravelTools()))
        .scenario("error", run -> ask(run, client(run).build(), Content.ERROR, new TravelTools()))
        .scenario("streaming", run -> {
          ChatClient chat = client(run).build();
          System.out.println(run.trace(() -> String.join("",
              chat.prompt().user(Content.STREAMING).tools(new TravelTools()).stream().content().collectList().block())));
        })
        .scenario("structured_output", run -> {
          ChatClient chat = client(run).build();
          System.out.println(run.trace(() -> chat.prompt().user(Content.STRUCTURED).call().entity(TripPlan.class)));
        })
        .scenario("files", run -> {
          ChatClient chat = client(run).build();
          System.out.println(run.trace(() -> chat.prompt()
              .user(u -> u.text(Content.FILES)
                  .media(MimeType.valueOf("image/jpeg"), new ByteArrayResource(Examples.asset("img.jpg")))
                  .media(MimeType.valueOf("application/pdf"), new ByteArrayResource(Examples.asset("task.pdf"))))
              .call().content()));
        })
        .main(args);
  }

  /** Arize's documented setup: the instrumentor is an observation handler on the application's registry. */
  private static void configure(Telemetry telemetry) {
    observations = ObservationRegistry.create();
    observations.observationConfig()
        .observationHandler(new SpringAIInstrumentor(new OITracer(telemetry.sdk.getTracer("com.arize.spring-ai"))));
  }
}
