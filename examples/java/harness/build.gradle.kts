plugins { `java-library` }

dependencies {
    api(platform(libs.opentelemetry.bom))
    api(libs.opentelemetry.sdk)
    implementation(libs.opentelemetry.exporter.otlp)
    api(libs.jackson.databind)
}
