plugins {
    alias(libs.plugins.kotlin.jvm) apply false
    alias(libs.plugins.kotlin.serialization) apply false
}

subprojects {
    // Keep parameter names in the bytecode: Spring AI and LangChain4j derive a tool's argument names from
    // its method's, and without this they become `arg0`, `arg1`.
    tasks.withType<JavaCompile>().configureEach { options.compilerArgs.add("-parameters") }
    plugins.withType<JavaPlugin> {
        extensions.configure<JavaPluginExtension> {
            toolchain { languageVersion.set(JavaLanguageVersion.of(25)) }
        }
    }
}
