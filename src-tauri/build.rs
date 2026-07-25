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

fn track_git_head() {
    let Ok(head) = std::fs::read_to_string("../.git/HEAD") else {
        return;
    };
    if let Some(reference) = head.trim().strip_prefix("ref: ") {
        println!("cargo:rerun-if-changed=../.git/{reference}");
    }
}

fn main() {
    println!("cargo:rerun-if-env-changed=DEVICEHUB_BUILD_NUMBER");
    println!("cargo:rerun-if-env-changed=DEVICEHUB_COMMIT");
    println!("cargo:rerun-if-env-changed=DEVICEHUB_UPDATE_CHANNEL");
    println!("cargo:rerun-if-env-changed=GITHUB_RUN_NUMBER");
    println!("cargo:rerun-if-env-changed=GITHUB_SHA");
    println!("cargo:rerun-if-changed=../.git/HEAD");
    track_git_head();

    let build =
        env_value("DEVICEHUB_BUILD_NUMBER", "GITHUB_RUN_NUMBER").unwrap_or_else(|| "dev".into());
    let channel = std::env::var("DEVICEHUB_UPDATE_CHANNEL")
        .ok()
        .filter(|value| matches!(value.as_str(), "stable" | "nightly"))
        .unwrap_or_else(|| "nightly".into());
    println!("cargo:rustc-env=DEVICEHUB_BUILD_NUMBER={build}");
    println!("cargo:rustc-env=DEVICEHUB_COMMIT={}", git_commit());
    println!("cargo:rustc-env=DEVICEHUB_UPDATE_CHANNEL={channel}");

    tauri_build::build()
}
