import javax.inject.Inject
import org.gradle.process.ExecOperations

plugins {
    id("com.android.application")
}

abstract class BuildRustLibrary @Inject constructor(
    private val execOperations: ExecOperations,
) : DefaultTask() {
    @get:Input
    abstract val androidAbi: Property<String>

    @get:InputFiles
    @get:PathSensitive(PathSensitivity.RELATIVE)
    abstract val sources: ConfigurableFileCollection

    @get:Internal
    abstract val repositoryRoot: DirectoryProperty

    @get:OutputDirectory
    abstract val outputDir: DirectoryProperty

    @get:InputFile
    @get:PathSensitive(PathSensitivity.NONE)
    abstract val androidJar: RegularFileProperty

    @TaskAction
    fun build() {
        execOperations.exec {
            environment("ANDROID_JAR", androidJar.get().asFile.absolutePath)
            workingDir(repositoryRoot.get().asFile)
            commandLine(
                "cargo", "ndk", "-t", androidAbi.get(), "--platform", "26",
                "-o", outputDir.get().asFile.absolutePath,
                "build", "--release", "--lib",
            )
        }
    }
}

val selectedAndroidAbi = providers.gradleProperty("kunianki.androidAbi").orElse("arm64-v8a")

val releaseSigningKeys = listOf(
    "KUNIANKI_KEYSTORE_PATH",
    "KUNIANKI_KEYSTORE_PASSWORD",
    "KUNIANKI_KEY_ALIAS",
    "KUNIANKI_KEY_PASSWORD",
)
val releaseSigningValues = releaseSigningKeys.associateWith { key ->
    providers.environmentVariable(key).orNull
}
val signingIsConfigured = releaseSigningValues.values.all { !it.isNullOrBlank() }
val signingIsPartiallyConfigured = releaseSigningValues.values.any { it != null } && !signingIsConfigured
if (signingIsPartiallyConfigured) {
    val missing = releaseSigningValues.filterValues { it.isNullOrBlank() }.keys
    throw GradleException(
        "Incomplete Android release signing configuration; set all four signing environment variables to non-empty values together. Missing or empty: ${missing.joinToString()}",
    )
}

val releaseSigning = if (signingIsConfigured) {
    releaseSigningValues.mapValues { requireNotNull(it.value) }
} else {
    null
}

val repoRoot = rootProject.layout.projectDirectory.dir("..")
val buildRustLibrary = tasks.register<BuildRustLibrary>("buildRustLibrary") {
    androidAbi.set(selectedAndroidAbi)
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

val compileSdkApi = 36

android {
    namespace = "dev.kunianki.app"
    compileSdk = compileSdkApi

    defaultConfig {
        applicationId = "dev.kunianki.app"
        minSdk = 26
        targetSdk = 36
        versionCode = 1
        versionName = "0.1.0"
        ndk {
            abiFilters += selectedAndroidAbi.get()
        }
    }

    buildTypes {
        getByName("release") {
            signingConfig = if (releaseSigning != null) {
                signingConfigs.create("release") {
                    storeFile = file(releaseSigning.getValue("KUNIANKI_KEYSTORE_PATH"))
                    storePassword = releaseSigning.getValue("KUNIANKI_KEYSTORE_PASSWORD")
                    keyAlias = releaseSigning.getValue("KUNIANKI_KEY_ALIAS")
                    keyPassword = releaseSigning.getValue("KUNIANKI_KEY_PASSWORD")
                }
            } else {
                null
            }
        }
    }

    sourceSets.getByName("main").res.srcDir(repoRoot.dir("assets/icons/android/res"))
}

val androidSdkRoot = providers.environmentVariable("ANDROID_HOME")
    .orElse(providers.environmentVariable("ANDROID_SDK_ROOT"))
    .get()
val androidSdkJar = file("$androidSdkRoot/platforms/android-$compileSdkApi/android.jar")

buildRustLibrary.configure {
    androidJar.set(androidSdkJar)
}

// AGP 9 rejects providers in the sourceSets DSL; register the Rust output as a generated jniLibs directory.
androidComponents {
    onVariants { variant ->
        variant.sources.jniLibs?.addGeneratedSourceDirectory(buildRustLibrary, BuildRustLibrary::outputDir)
    }
}

