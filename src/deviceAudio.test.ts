import { describe, expect, it } from "vitest";
import { deviceAudioControlAction, parseLegacyDeviceAudioPreferences } from "./deviceAudio";

describe("device audio", () => {
  it("prioritizes enable and mute state", () => {
    expect(deviceAudioControlAction(null, false)).toBe("unavailable");
    expect(deviceAudioControlAction(false, false)).toBe("enable");
    expect(deviceAudioControlAction(true, true)).toBe("unmute");
    expect(deviceAudioControlAction(true, false)).toBe("mute");
  });

  it("validates and clamps legacy Web Audio preferences", () => {
    expect(parseLegacyDeviceAudioPreferences(null)).toBeNull();
    expect(parseLegacyDeviceAudioPreferences("broken")).toBeNull();
    expect(parseLegacyDeviceAudioPreferences('{"muted":true,"volume":4}')).toEqual({
      muted: true,
      volume: 1,
    });
  });
});
