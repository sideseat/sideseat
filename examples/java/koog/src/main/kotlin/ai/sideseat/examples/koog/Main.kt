// The Koog suite: the shared scenarios on Koog agents, tools and its OpenTelemetry feature.
//
// Koog's OpenTelemetry feature records each agent run, model call and tool execution as spans with
// the current OpenTelemetry GenAI conventions - `gen_ai.input.messages`, `gen_ai.output.messages`,
// `gen_ai.system_instructions`. It builds an OpenTelemetry SDK of its own and roots each run's trace
// at the agent, so the session and user are stamped on its spans by a span adapter, the hook Koog
// documents for enriching them, rather than by an enclosing span it would not see. The model is Claude on Amazon Bedrock through
// Koog's own Bedrock client, sent to the capture proxy when one runs.
package ai.sideseat.examples.koog

import ai.koog.agents.core.agent.AIAgent
import ai.koog.agents.core.tools.ToolException
import ai.koog.agents.core.tools.ToolRegistry
import ai.koog.agents.core.tools.annotations.LLMDescription
import ai.koog.agents.core.tools.annotations.Tool
import ai.koog.agents.core.tools.reflect.ToolSet
import ai.koog.agents.features.opentelemetry.attribute.CustomAttribute
import ai.koog.agents.features.opentelemetry.feature.OpenTelemetry
import ai.koog.agents.features.opentelemetry.integration.SpanAdapter
import ai.koog.agents.features.opentelemetry.span.GenAIAgentSpan
import ai.koog.prompt.executor.clients.anthropic.AnthropicModels
import ai.koog.prompt.executor.clients.bedrock.BedrockAPIMethod
import ai.koog.prompt.executor.clients.bedrock.BedrockClientSettings
import ai.koog.prompt.executor.clients.bedrock.BedrockLLMClient
import ai.koog.prompt.executor.clients.bedrock.BedrockModel
import ai.koog.prompt.executor.llms.MultiLLMPromptExecutor
import ai.koog.prompt.llm.LLModel
import ai.sideseat.examples.harness.Content
import ai.sideseat.examples.harness.Model
import ai.sideseat.examples.harness.Run
import ai.sideseat.examples.harness.Sample
import ai.sideseat.examples.harness.Telemetry
import ai.sideseat.examples.harness.Tools
import aws.smithy.kotlin.runtime.auth.awscredentials.Credentials
import aws.smithy.kotlin.runtime.auth.awscredentials.CredentialsProvider
import aws.smithy.kotlin.runtime.collections.Attributes
import aws.sdk.kotlin.runtime.auth.credentials.DefaultChainCredentialsProvider
import com.fasterxml.jackson.databind.ObjectMapper
import kotlinx.coroutines.runBlocking

private val json = ObjectMapper()
private lateinit var telemetry: Telemetry

/** The proxy discards a client's signature, so a client pointed at it needs no credentials of its own. */
private object ProxyCredentials : CredentialsProvider {
    override suspend fun resolve(attributes: Attributes) = Credentials("proxy", "proxy")
}

private fun executor(run: Run): Pair<MultiLLMPromptExecutor, LLModel> {
    val model = run.model.require("Koog", "bedrock")
    val proxy = Model.modelProxy()
    val client = BedrockLLMClient(
        identityProvider = if (proxy != null) ProxyCredentials else DefaultChainCredentialsProvider(),
        settings = BedrockClientSettings(region = Model.region(), endpointUrl = proxy, apiMethod = BedrockAPIMethod.Converse, maxRetries = 0),
    )
    // Koog's catalogue names the model family; the id is the harness's, inference profile included.
    val (prefix, id) = model.id().split('.', limit = 2).let { it[0] to it[1] }
    return MultiLLMPromptExecutor(listOf(client)) to BedrockModel(AnthropicModels.Sonnet_5, id, prefix).effectiveModel
}

/** The shared tools, with the shared definitions. Public, because Koog calls tool methods by reflection. */
@LLMDescription("Travel tools")
class TravelTools : ToolSet {
    @Tool("get_weather")
    @LLMDescription("Get the weather forecast for a city.")
    fun getWeather(
        @LLMDescription("The city name.") city: String,
        @LLMDescription("How many days to forecast, from 1 to 7.") days: Int = 1,
    ): String = Tools.resultText("get_weather", json.createObjectNode().put("city", city).put("days", days))

    @Tool("get_precipitation")
    @LLMDescription("Get the chance of rain in a city for tomorrow.")
    fun getPrecipitation(@LLMDescription("The city name.") city: String): String =
        Tools.resultText("get_precipitation", json.createObjectNode().put("city", city))

    @Tool("book_flight")
    @LLMDescription("Book a flight.")
    fun bookFlight(
        @LLMDescription("Departure city.") origin: String,
        @LLMDescription("Arrival city.") destination: String,
        @LLMDescription("Travel date.") date: String,
    ): String {
        val args = json.createObjectNode().put("origin", origin).put("destination", destination).put("date", date)
        // The model reads the failure and answers anyway: Koog relays a tool exception's message.
        throw ToolException.ValidationFailure(Tools.resultText("book_flight", args))
    }
}

private fun registry(vararg names: String): ToolRegistry {
    val all = TravelTools().asTools().associateBy { it.name }
    return ToolRegistry { names.forEach { tool(all.getValue(it)) } }
}

/** Stamps the scenario's session and user on every span Koog records. */
private class Correlation(private val run: Run) : SpanAdapter() {
    override fun onBeforeSpanStarted(span: GenAIAgentSpan) {
        span.addAttribute(CustomAttribute("session.id", run.sessionId()))
        span.addAttribute(CustomAttribute("user.id", Content.USER_ID))
    }
}

private fun agent(run: Run, tools: ToolRegistry = ToolRegistry.EMPTY, system: String = Content.SYSTEM): AIAgent<String, String> {
    val (executor, model) = executor(run)
    return AIAgent(promptExecutor = executor, llmModel = model, toolRegistry = tools, systemPrompt = system) {
        // Koog's documented setup: an OTLP exporter, message content recorded, flushed when the agent closes.
        install(OpenTelemetry) {
            setServiceInfo(telemetry.serviceName, "1.0.0")
            addResourceAttributes(telemetry.resourceAttributes())
            addSpanExporter(telemetry.spanExporter())
            addSpanAdapter(Correlation(run))
            setVerbose(true)
        }
    }
}

private fun runAgent(agent: AIAgent<String, String>, question: String): String =
    runBlocking { agent.run(question).also { agent.close() } }

private fun ask(agent: AIAgent<String, String>, question: String) = println(runAgent(agent, question))

/** A specialist agent with the forecast tools, offered to a coordinator as a tool. */
@LLMDescription("Specialists")
class Specialist(private val run: Run) : ToolSet {
    @Tool("weather_specialist")
    @LLMDescription("Ask the weather specialist, which has the forecast tools, a question.")
    fun ask(@LLMDescription("The question for the specialist.") request: String): String =
        runAgent(agent(run, registry("get_weather", "get_precipitation")), request)
}

fun main(args: Array<String>) {
    Sample()
        .configure { telemetry = it }
        .scenario("chat") { run -> ask(agent(run), Content.CHAT) }
        .scenario("tool_use") { run -> ask(agent(run, registry("get_weather", "get_precipitation")), Content.TOOL_USE) }
        .scenario("error") { run -> ask(agent(run, registry("book_flight")), Content.ERROR) }
        .scenario("session") { run ->
            // Two traces, each its own agent run, attributed to one session and user.
            Content.SESSION.forEach { question -> ask(agent(run), question) }
        }
        .scenario("multi_agent") { run ->
            // A coordinator that hands the forecast to a specialist agent, which runs under the tool call.
            val coordinator = agent(run, ToolRegistry { tools(Specialist(run)) },
                Content.SYSTEM + " Ask weather_specialist for any forecast you need.")
            ask(coordinator, Content.MULTI_AGENT)
        }
        .main(args)
}
