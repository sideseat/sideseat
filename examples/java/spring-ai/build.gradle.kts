plugins { application }

dependencies {
    implementation(project(":harness"))
    implementation(libs.spring.ai.bedrock.converse)
    implementation(libs.spring.ai.client.chat)
    implementation(platform(libs.awssdk.bom))
    implementation(libs.awssdk.bedrockruntime)
    implementation(libs.awssdk.netty)
    implementation(libs.openinference.springai)
}

application { mainClass.set("ai.sideseat.examples.springai.Main") }

tasks.named<JavaExec>("run") { workingDir = projectDir }
