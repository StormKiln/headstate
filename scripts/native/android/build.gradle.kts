import org.gradle.api.artifacts.dsl.LockMode
buildscript {
    repositories { google(); mavenCentral() }
    dependencies {
        classpath("com.android.tools.build:gradle:8.11.0")
        classpath("org.jetbrains.kotlin:kotlin-gradle-plugin:1.9.25")
    }
    configurations.getByName("classpath") { resolutionStrategy.activateDependencyLocking() }
}
allprojects {
    repositories { google(); mavenCentral() }
    dependencyLocking {
        lockAllConfigurations()
        lockMode.set(LockMode.STRICT)
    }
}
