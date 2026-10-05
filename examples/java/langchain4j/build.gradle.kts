plugins { application }

dependencies {
    implementation(project(":harness"))
    implementation(libs.langchain4j)
    implementation(libs.langchain4j.bedrock)
    implementation(platform(libs.awssdk.bom))
    implementation(libs.awssdk.bedrockruntime)
    implementation(libs.openinference.langchain4j)
}

application { mainClass.set("ai.sideseat.examples.langchain4j.Main") }

tasks.named<JavaExec>("run") { workingDir = projectDir }
