plugins {
    application
    alias(libs.plugins.kotlin.jvm)
    alias(libs.plugins.kotlin.serialization)
}

kotlin { jvmToolchain(25) }

dependencies {
    implementation(project(":harness"))
    implementation(libs.koog.agents)
    implementation(libs.koog.opentelemetry)
    implementation(libs.koog.bedrock)
}

application { mainClass.set("ai.sideseat.examples.koog.MainKt") }

tasks.named<JavaExec>("run") { workingDir = projectDir }
