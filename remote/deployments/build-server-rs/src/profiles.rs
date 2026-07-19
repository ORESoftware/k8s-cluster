//! Fixed, operator-reviewed CI profiles.
//!
//! API callers select a profile name; they never supply commands or runner
//! images. Repository code is still executable and therefore remains limited to
//! trusted, allowlisted repositories, exactly like Dockerfile builds.

use serde::Serialize;

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ProfileStep {
    pub name: &'static str,
    pub image: &'static str,
    pub subdirectory: &'static str,
    #[serde(skip_serializing)]
    pub script: &'static str,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ProfileSpec {
    pub name: &'static str,
    pub platform: &'static str,
    pub description: &'static str,
    pub steps: &'static [ProfileStep],
    pub artifact_paths: &'static [&'static str],
}

const FLUTTER_IMAGE: &str =
    "710156900967.dkr.ecr.us-east-1.amazonaws.com/sonus-flutter-builder:3.44.2-c9a6c48423";
const BROWSER_IMAGE: &str = "mcr.microsoft.com/playwright:v1.60.0-noble";

const FLUTTER_VERIFY_STEPS: &[ProfileStep] = &[ProfileStep {
    name: "flutter verify",
    image: FLUTTER_IMAGE,
    subdirectory: ".",
    script: "flutter pub get && flutter analyze --no-fatal-infos && flutter test",
}];

const FLUTTER_ANDROID_STEPS: &[ProfileStep] = &[ProfileStep {
    name: "flutter Android debug",
    image: FLUTTER_IMAGE,
    subdirectory: ".",
    script: "flutter pub get && flutter analyze --no-fatal-infos && flutter test && flutter build apk --debug --dart-define=SONUS_BACKEND_BASE_URL=https://ci.invalid --dart-define=SONUS_SUPABASE_URL=https://ci.supabase.co --dart-define=SONUS_SUPABASE_ANON_KEY=sb_publishable_ci_compile_only",
}];

const FLUTTER_WEB_STEPS: &[ProfileStep] = &[ProfileStep {
    name: "flutter web release",
    image: FLUTTER_IMAGE,
    subdirectory: ".",
    script: "flutter pub get && flutter analyze --no-fatal-infos && flutter test && flutter build web --release",
}];

const FLUTTER_LINUX_STEPS: &[ProfileStep] = &[ProfileStep {
    name: "flutter Linux release",
    image: FLUTTER_IMAGE,
    subdirectory: ".",
    script: "flutter config --enable-linux-desktop && flutter pub get && flutter analyze --no-fatal-infos && flutter test && flutter build linux --release",
}];

const FLUTTER_LINUX_DESKTOP_ENTRYPOINT_STEPS: &[ProfileStep] = &[ProfileStep {
    name: "Flutter Linux desktop entrypoint release",
    image: FLUTTER_IMAGE,
    subdirectory: ".",
    script: "flutter config --enable-linux-desktop && flutter pub get && flutter analyze --no-fatal-infos && flutter test && flutter build linux --release -t lib/main_desktop.dart --dart-define=SONUS_BACKEND_BASE_URL=https://ci.invalid --dart-define=SONUS_SUPABASE_URL=https://ci.supabase.co --dart-define=SONUS_SUPABASE_ANON_KEY=sb_publishable_ci_compile_only",
}];

const FLUTTER_WEB_E2E_STEPS: &[ProfileStep] = &[
    ProfileStep {
        name: "flutter web release",
        image: FLUTTER_IMAGE,
        subdirectory: ".",
        script: "flutter pub get && flutter analyze --no-fatal-infos && flutter test && flutter build web --release",
    },
    ProfileStep {
        name: "Puppeteer and Playwright end-to-end tests",
        image: BROWSER_IMAGE,
        subdirectory: "e2e",
        script: "npm ci && npm test",
    },
];

const PLAYWRIGHT_STEPS: &[ProfileStep] = &[ProfileStep {
    name: "Playwright tests",
    image: BROWSER_IMAGE,
    subdirectory: ".",
    script: "npm ci && npx playwright test",
}];

const PUPPETEER_STEPS: &[ProfileStep] = &[ProfileStep {
    name: "Puppeteer tests",
    image: BROWSER_IMAGE,
    subdirectory: ".",
    script: "npm ci && npm run test:puppeteer",
}];

const BROWSER_E2E_STEPS: &[ProfileStep] = &[ProfileStep {
    name: "browser end-to-end tests",
    image: BROWSER_IMAGE,
    subdirectory: ".",
    script: "npm ci && npm test",
}];

pub const SPECS: &[ProfileSpec] = &[
    ProfileSpec {
        name: "flutter-verify",
        platform: "linux",
        description: "Flutter dependency resolution, analysis, and unit tests",
        steps: FLUTTER_VERIFY_STEPS,
        artifact_paths: &[],
    },
    ProfileSpec {
        name: "flutter-android-debug",
        platform: "linux",
        description: "Flutter verification plus an Android debug APK",
        steps: FLUTTER_ANDROID_STEPS,
        artifact_paths: &["build/app/outputs/flutter-apk/app-debug.apk"],
    },
    ProfileSpec {
        name: "flutter-web-release",
        platform: "linux",
        description: "Flutter verification plus a release web bundle",
        steps: FLUTTER_WEB_STEPS,
        artifact_paths: &["build/web"],
    },
    ProfileSpec {
        name: "flutter-linux-release",
        platform: "linux",
        description: "Flutter verification plus a native Linux desktop bundle",
        steps: FLUTTER_LINUX_STEPS,
        artifact_paths: &["build/linux"],
    },
    ProfileSpec {
        name: "flutter-linux-desktop-entrypoint",
        platform: "linux",
        description: "Flutter native Linux bundle using lib/main_desktop.dart",
        steps: FLUTTER_LINUX_DESKTOP_ENTRYPOINT_STEPS,
        artifact_paths: &["build/linux"],
    },
    ProfileSpec {
        name: "flutter-web-e2e",
        platform: "linux",
        description:
            "Flutter web release followed by the repository's Puppeteer and Playwright suite",
        steps: FLUTTER_WEB_E2E_STEPS,
        artifact_paths: &["build/web", "e2e/artifacts"],
    },
    ProfileSpec {
        name: "playwright",
        platform: "linux",
        description: "Node project Playwright suite",
        steps: PLAYWRIGHT_STEPS,
        artifact_paths: &[],
    },
    ProfileSpec {
        name: "puppeteer",
        platform: "linux",
        description: "Node project test:puppeteer script",
        steps: PUPPETEER_STEPS,
        artifact_paths: &[],
    },
    ProfileSpec {
        name: "browser-e2e",
        platform: "linux",
        description: "Node project's default test suite in a browser-ready image",
        steps: BROWSER_E2E_STEPS,
        artifact_paths: &[],
    },
];

pub fn find(name: &str) -> Option<&'static ProfileSpec> {
    SPECS.iter().find(|profile| profile.name == name)
}

pub fn names() -> impl Iterator<Item = &'static str> {
    SPECS.iter().map(|profile| profile.name)
}
