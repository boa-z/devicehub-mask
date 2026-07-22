import { invoke } from "@tauri-apps/api/core";

export type VideoPixelFormat = "rgb24" | "yuv420p";

export type VideoSettingsStatus = {
  video_pixel_format: VideoPixelFormat;
  environment_override: boolean;
};

export function readVideoSettings() {
  return invoke<VideoSettingsStatus>("video_settings_status");
}

export function setVideoPixelFormat(videoPixelFormat: VideoPixelFormat) {
  return invoke<VideoSettingsStatus>("set_video_pixel_format", {
    videoPixelFormat,
  });
}
