plugins {
    id("com.android.application")
}

val repositoryRoot = rootProject.projectDir.parentFile
val rustJniLibs = layout.buildDirectory.dir("generated/rustJniLibs")
val buildRustLibrary = tasks.register<Exec>("buildRustLibrary") {
    inputs.files(
        fileTree(repositoryRoot.resolve("src")),
        fileTree(repositoryRoot.resolve("ui")),
        fileTree(repositoryRoot.resolve("data")),
        fileTree(repositoryRoot.resolve("assets")),
        repositoryRoot.resolve("Cargo.toml"),
        repositoryRoot.resolve("Cargo.lock"),
        repositoryRoot.resolve("build.rs"),
    )
    outputs.dir(rustJniLibs)
    workingDir(repositoryRoot)
    commandLine(
        "cargo", "ndk", "-t", "arm64-v8a", "--platform", "26",
        "-o", rustJniLibs.get().asFile.absolutePath,
        "build", "--release", "--lib",
    )
}

android {
    namespace = "dev.kunianki.app"
    compileSdk = 35

    defaultConfig {
        applicationId = "dev.kunianki.app"
        minSdk = 26
        targetSdk = 35
        ndk {
            abiFilters += "arm64-v8a"
        }
    }

    sourceSets.getByName("main").jniLibs.srcDir(rustJniLibs)
}

tasks.named("preBuild").configure {
    dependsOn(buildRustLibrary)
}
