plugins { application }

dependencies {
    implementation(project(":harness"))
    implementation(libs.adk)
    implementation(libs.anthropic.bedrock)
}

application { mainClass.set("ai.sideseat.examples.adk.Main") }

tasks.named<JavaExec>("run") { workingDir = projectDir }
