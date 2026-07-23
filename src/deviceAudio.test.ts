import { describe, expect, it } from "vitest";
import { PcmAudioPlayer, defaultDeviceAudioPreferences, deviceAudioControlAction, parseAudioEnvelope, parseDeviceAudioPreferences, shouldAttemptAudioResume } from "./deviceAudio";

describe("device audio", () => {
  it("parses PCM envelopes and rejects ordinary image data", () => {
    expect(parseAudioEnvelope(new Uint8Array([0xff, 0xd8, 0xff]).buffer)).toBeNull();
    const bytes = new Uint8Array(24);
    bytes.set([0x44, 0x48, 0x41, 0x50, 1, 1, 2, 0]);
    const view = new DataView(bytes.buffer);
    view.setUint32(8, 48_000, true);
    view.setUint32(12, 2, true);
    view.setInt16(16, 100, true);
    view.setInt16(18, -100, true);
    view.setInt16(20, 200, true);
    view.setInt16(22, -200, true);
    const chunk = parseAudioEnvelope(bytes.buffer);
    expect(chunk && [...chunk.samples]).toEqual([100, -100, 200, -200]);
  });

  it("defaults and clamps playback preferences", () => {
    expect(parseDeviceAudioPreferences(null)).toEqual(defaultDeviceAudioPreferences);
    expect(parseDeviceAudioPreferences("broken")).toEqual(defaultDeviceAudioPreferences);
    expect(parseDeviceAudioPreferences('{"muted":true,"volume":4}')).toEqual({ muted: true, volume: 1 });
  });

  it("prioritizes enable, unmute, and resume before mute", () => {
    expect(deviceAudioControlAction(null, false, false)).toBe("unavailable");
    expect(deviceAudioControlAction(false, false, false)).toBe("enable");
    expect(deviceAudioControlAction(true, true, true)).toBe("unmute");
    expect(deviceAudioControlAction(true, false, true)).toBe("resume");
    expect(deviceAudioControlAction(true, false, false)).toBe("mute");
  });

  it("only resumes enabled, audible playback that is not already running", () => {
    expect(shouldAttemptAudioResume(null, false, false)).toBe(false);
    expect(shouldAttemptAudioResume(false, false, false)).toBe(false);
    expect(shouldAttemptAudioResume(true, true, false)).toBe(false);
    expect(shouldAttemptAudioResume(true, false, true)).toBe(false);
    expect(shouldAttemptAudioResume(true, false, false)).toBe(true);
  });

  it("treats muted and zero-volume playback as inaudible", () => {
    const muted = new PcmAudioPlayer({ muted: true, volume: 1 });
    const silent = new PcmAudioPlayer({ muted: false, volume: 0 });
    const audible = new PcmAudioPlayer({ muted: false, volume: 0.5 });
    expect(muted.isAudible()).toBe(false);
    expect(silent.isAudible()).toBe(false);
    expect(audible.isAudible()).toBe(true);
  });
});
