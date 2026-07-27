use std::process::Command;

fn env_value(primary: &str, fallback: &str) -> Option<String> {
    std::env::var(primary)
        .ok()
        .filter(|value| !value.trim().is_empty())
        .or_else(|| {
            std::env::var(fallback)
                .ok()
                .filter(|value| !value.trim().is_empty())
        })
}

fn git_commit() -> String {
    env_value("DEVICEHUB_COMMIT", "GITHUB_SHA")
        .or_else(|| {
            Command::new("git")
                .args(["rev-parse", "--short=7", "HEAD"])
                .output()
                .ok()
                .filter(|output| output.status.success())
                .and_then(|output| String::from_utf8(output.stdout).ok())
        })
        .map(|value| value.trim().chars().take(7).collect())
        .filter(|value: &String| !value.is_empty())
        .unwrap_or_else(|| "unknown".into())
}

fn main() {
    for variable in [
        "DEVICEHUB_BUILD_NUMBER",
        "DEVICEHUB_COMMIT",
        "DEVICEHUB_UPDATE_CHANNEL",
        "GITHUB_RUN_NUMBER",
        "GITHUB_SHA",
    ] {
        println!("cargo:rerun-if-env-changed={variable}");
    }
    let build =
        env_value("DEVICEHUB_BUILD_NUMBER", "GITHUB_RUN_NUMBER").unwrap_or_else(|| "dev".into());
    let channel = std::env::var("DEVICEHUB_UPDATE_CHANNEL")
        .ok()
        .filter(|value| matches!(value.as_str(), "stable" | "nightly"))
        .unwrap_or_else(|| "nightly".into());
    println!("cargo:rustc-env=DEVICEHUB_BUILD_NUMBER={build}");
    println!("cargo:rustc-env=DEVICEHUB_COMMIT={}", git_commit());
    println!("cargo:rustc-env=DEVICEHUB_UPDATE_CHANNEL={channel}");
}
