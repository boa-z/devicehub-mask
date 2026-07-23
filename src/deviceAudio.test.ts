import { afterEach, describe, expect, it, vi } from "vitest";
import { PcmAudioPlayer, defaultDeviceAudioPreferences, deviceAudioControlAction, parseAudioEnvelope, parseDeviceAudioPreferences, shouldAttemptAudioResume, shouldAttemptAudioResumeOnLifecycle, shouldReuseAudioResumeAttempt } from "./deviceAudio";

describe("device audio", () => {
  afterEach(() => vi.unstubAllGlobals());

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
    expect(shouldAttemptAudioResumeOnLifecycle(true, false, false, "visible")).toBe(true);
    expect(shouldAttemptAudioResumeOnLifecycle(true, false, false, "hidden")).toBe(false);
  });

  it("reuses a gesture attempt but supersedes an automatic attempt", () => {
    expect(shouldReuseAudioResumeAttempt(false, false)).toBe(true);
    expect(shouldReuseAudioResumeAttempt(false, true)).toBe(false);
    expect(shouldReuseAudioResumeAttempt(true, false)).toBe(true);
    expect(shouldReuseAudioResumeAttempt(true, true)).toBe(true);
  });

  it("waits for user activation before creating a context and replaces a stuck context", async () => {
    const contexts: FakeAudioContext[] = [];
    class TestAudioContext extends FakeAudioContext {
      constructor() {
        super();
        contexts.push(this);
      }
    }
    vi.stubGlobal("AudioContext", TestAudioContext);
    const player = new PcmAudioPlayer(defaultDeviceAudioPreferences);
    const chunk = { sampleRate: 48_000, channels: 2, frames: 1, samples: new Int16Array(2) };

    expect(player.isAudible()).toBe(true);
    expect(player.contextState()).toBe("uninitialized");
    expect(player.push(chunk)).toBe(false);
    expect(contexts).toHaveLength(0);
    expect(await player.resume(true)).toBe(true);
    expect(contexts).toHaveLength(1);
    expect(contexts[0].startedSources).toBe(1);
    expect(player.contextState()).toBe("running");

    contexts[0].state = "suspended";
    expect(await player.resume(true)).toBe(true);
    expect(contexts).toHaveLength(2);
    expect(contexts[0].closed).toBe(true);
    player.close();
  });

  it("reports muted and zero-volume playback as inaudible", () => {
    expect(new PcmAudioPlayer({ muted: true, volume: 1 }).isAudible()).toBe(false);
    expect(new PcmAudioPlayer({ muted: false, volume: 0 }).isAudible()).toBe(false);
    expect(new PcmAudioPlayer({ muted: false, volume: 0.5 }).isAudible()).toBe(true);
  });
});

class FakeAudioContext {
  state: AudioContextState = "suspended";
  currentTime = 0;
  destination = {} as AudioDestinationNode;
  closed = false;
  sampleRate = 48_000;
  startedSources = 0;

  createGain() {
    return {
      gain: { setValueAtTime: vi.fn() },
      connect: vi.fn(),
    } as unknown as GainNode;
  }

  createBuffer() {
    return {} as AudioBuffer;
  }

  createBufferSource() {
    return {
      buffer: null,
      connect: vi.fn(),
      start: vi.fn(() => { this.startedSources += 1; }),
      stop: vi.fn(),
      onended: null,
    } as unknown as AudioBufferSourceNode;
  }

  async resume() {
    this.state = "running";
  }

  async close() {
    this.closed = true;
    this.state = "closed";
  }
}
