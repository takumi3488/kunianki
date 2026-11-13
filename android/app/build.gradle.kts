import javax.inject.Inject
import org.gradle.process.ExecOperations

plugins {
    id("com.android.application")
}

abstract class BuildRustLibrary @Inject constructor(
    private val execOperations: ExecOperations,
) : DefaultTask() {
    @get:InputFiles
    @get:PathSensitive(PathSensitivity.RELATIVE)
    abstract val sources: ConfigurableFileCollection

    @get:Internal
    abstract val repositoryRoot: DirectoryProperty

    @get:OutputDirectory
    abstract val outputDir: DirectoryProperty

    @TaskAction
    fun build() {
        execOperations.exec {
            workingDir(repositoryRoot.get().asFile)
            commandLine(
                "cargo", "ndk", "-t", "arm64-v8a", "--platform", "26",
                "-o", outputDir.get().asFile.absolutePath,
                "build", "--release", "--lib",
            )
        }
    }
}

val repoRoot = rootProject.layout.projectDirectory.dir("..")
val buildRustLibrary = tasks.register<BuildRustLibrary>("buildRustLibrary") {
    repositoryRoot.set(repoRoot)
    sources.from(
        repoRoot.dir("src"),
        repoRoot.dir("ui"),
        repoRoot.dir("data"),
        repoRoot.dir("assets"),
        repoRoot.file("Cargo.toml"),
        repoRoot.file("Cargo.lock"),
        repoRoot.file("build.rs"),
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
}

// AGP 9 rejects providers in the sourceSets DSL; register the Rust output as a generated jniLibs directory.
androidComponents {
    onVariants { variant ->
        variant.sources.jniLibs?.addGeneratedSourceDirectory(buildRustLibrary, BuildRustLibrary::outputDir)
    }
}
