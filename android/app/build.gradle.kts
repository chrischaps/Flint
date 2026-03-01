import java.util.Properties

plugins {
    id("com.android.application")
}

// ---- Load local.properties (sdk.dir, ndk.dir) ----
val localProps = Properties().apply {
    val f = rootProject.file("local.properties")
    if (f.exists()) f.inputStream().use { load(it) }
}
val ndkDir: String = localProps.getProperty("ndk.dir")
    ?: System.getenv("ANDROID_NDK_HOME")
    ?: error("Set ndk.dir in local.properties or ANDROID_NDK_HOME env var")

// ---- Load game.properties from the game project ----
val engineDir = rootProject.projectDir.parentFile  // engine/

val gameDirProp = if (project.hasProperty("gameDir")) project.property("gameDir") as String else ""
val gameDir = if (gameDirProp.isNotEmpty()) {
    file(gameDirProp)
} else {
    engineDir.parentFile  // Assume game project is parent of engine/
}

val gameProps = Properties().apply {
    val f = File(gameDir, "game.properties")
    if (f.exists()) f.inputStream().use { load(it) }
}

val gamePackage = gameProps.getProperty("game.package", "com.flint.game")
val gameName = gameProps.getProperty("game.name", "Flint Game")
val gameVersionName = gameProps.getProperty("game.version_name", "0.1.0")
val gameVersionCode = gameProps.getProperty("game.version_code", "1").toInt()
val gameIconDir = gameProps.getProperty("game.icon_dir", "")

android {
    namespace = gamePackage
    compileSdk = 35

    defaultConfig {
        applicationId = gamePackage
        minSdk = 26          // AAudio (Kira audio) + Vulkan
        targetSdk = 34
        versionCode = gameVersionCode
        versionName = gameVersionName

        // Inject app name as a string resource (replaces strings.xml)
        resValue("string", "app_name", gameName)

        ndk {
            abiFilters += listOf("arm64-v8a")
        }
    }

    buildTypes {
        release {
            isMinifyEnabled = false
        }
    }

    sourceSets {
        getByName("main") {
            // Native .so files built by cargo-ndk
            jniLibs.srcDirs("src/main/jniLibs")
        }
    }
}

// No Java dependencies needed — NativeActivity is built into Android.

// ---- Cargo NDK Build Task ----

tasks.register<Exec>("cargoNdkBuild") {
    description = "Build flint-android native library via cargo-ndk"
    workingDir = engineDir

    // Pass NDK path from local.properties so shell env vars aren't needed
    environment("ANDROID_NDK_HOME", ndkDir)

    val target = "arm64-v8a"
    val jniLibsDir = file("src/main/jniLibs")

    // Pass a unique build timestamp so the asset extractor always re-extracts.
    // This defeats Android Auto Backup restoring stale assets after reinstall.
    environment("FLINT_APK_VERSION", System.currentTimeMillis().toString())

    commandLine(
        "cargo", "ndk",
        "-t", target,
        "--platform", "26",
        "-o", jniLibsDir.absolutePath,
        "build", "--release",
        "-p", "flint-android"
    )
}

// ---- Copy Game Assets Task ----

tasks.register<Copy>("copyGameAssets") {
    description = "Copy game assets into APK assets directory"
    val assetsDir = file("src/main/assets")

    // Clean previous assets
    doFirst {
        assetsDir.deleteRecursively()
        assetsDir.mkdirs()
    }

    // Copy game-level files
    from(gameDir) {
        include("**/*.toml")
        include("**/*.png")
        include("**/*.jpg")
        include("**/*.ogg")
        include("**/*.wav")
        include("**/*.rhai")
        include("**/*.glb")
        include("**/*.gltf")
        // Exclude engine internals and build artifacts
        exclude("engine/**")
        exclude("target/**")
        exclude("android/**")
        exclude("build/**")
        exclude(".git/**")
    }

    // Copy engine schemas
    from(engineDir.resolve("schemas")) {
        into("engine/schemas")
    }

    into(assetsDir)

    // Generate a manifest of all asset files for the Rust extractor.
    doLast {
        val manifest = File(assetsDir, "asset_manifest.txt")
        val lines = mutableListOf<String>()
        assetsDir.walkTopDown().filter { it.isFile && it.name != "asset_manifest.txt" }.forEach {
            lines.add(it.relativeTo(assetsDir).path.replace("\\", "/"))
        }
        manifest.writeText(lines.joinToString("\n"))
    }
}

// ---- Copy Game Icons Task ----

tasks.register<Copy>("copyGameIcons") {
    description = "Copy game launcher icons into Android res/"

    val iconSrcDir = if (gameIconDir.isNotEmpty()) File(gameDir, gameIconDir) else null

    // Only run if game specifies an icon directory and it exists
    enabled = iconSrcDir != null && iconSrcDir.exists()

    if (iconSrcDir != null && iconSrcDir.exists()) {
        from(iconSrcDir) {
            include("mipmap-*/ic_launcher.png")
        }
        into(file("src/main/res"))
    }
}

// ---- Copy APK to Game Directory Task ----

tasks.register<Copy>("copyApkToGame") {
    description = "Copy built APK to game project build/ directory"

    val apkFile = file("build/outputs/apk/debug/app-debug.apk")
    // Derive APK name from game name: lowercase, hyphens for spaces
    val apkName = gameName.lowercase().replace(" ", "-") + ".apk"
    val outputDir = File(gameDir, "build")

    from(apkFile)
    into(outputDir)
    rename { apkName }
}

// Wire tasks as preBuild dependencies
tasks.named("preBuild") {
    dependsOn("cargoNdkBuild", "copyGameAssets", "copyGameIcons")
}

// Copy APK after assembly completes
tasks.whenTaskAdded {
    if (name == "assembleDebug" || name == "assembleRelease") {
        finalizedBy("copyApkToGame")
    }
}
