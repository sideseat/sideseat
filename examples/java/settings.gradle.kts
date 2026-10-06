// The JVM example suites: one Gradle build, the shared harness and one subproject per framework.
rootProject.name = "sideseat-examples-java"

plugins {
    id("org.gradle.toolchains.foojay-resolver-convention") version "1.0.0"
}

dependencyResolutionManagement {
    repositories { mavenCentral() }
}

include("harness")
include("adk")
include("koog")
include("langchain4j")
include("spring-ai")
