import { describe, expect, it, vi } from "vitest";
import { BrowserPcmPlayer, parseBrowserAudioPacket, type BrowserAudioPacket, type BrowserAudioPlaybackState } from "./browserAudio";

function packet(payload: number[]) {
  const bytes = new Uint8Array(12 + payload.length);
  bytes.set([0x44, 0x48, 0x41, 0x31]);
  const view = new DataView(bytes.buffer);
  view.setUint32(4, 48_000);
  view.setUint16(8, 2);
  bytes.set(payload, 12);
  return bytes.buffer;
}

describe("browser PCM packets", () => {
  it("parses the bounded audio header and little-endian PCM payload", () => {
    const parsed = parseBrowserAudioPacket(packet([1, 0, 2, 0]));
    expect(parsed?.sampleRate).toBe(48_000);
    expect(parsed?.channels).toBe(2);
    expect(parsed?.pcm.getInt16(0, true)).toBe(1);
    expect(parsed?.pcm.getInt16(2, true)).toBe(2);
  });

  it("distinguishes video packets and rejects incomplete audio frames", () => {
    expect(parseBrowserAudioPacket(new Uint8Array([0x44, 0x48, 0x56, 0x32]).buffer)).toBeNull();
    expect(() => parseBrowserAudioPacket(packet([1, 2]))).toThrow("PCM payload length");
  });
});

function mockAudioContext(resumeError?: Error) {
  let state: AudioContextState = "suspended";
  const createBuffer = vi.fn(() => ({
    duration: 0.02,
    getChannelData: () => new Float32Array(2),
  }));
  const context = {
    get state() { return state; },
    currentTime: 1,
    destination: {},
    onstatechange: null,
    createGain: () => ({
      gain: { value: 1, setValueAtTime: vi.fn() },
      connect: vi.fn(),
    }),
    createBuffer,
    createBufferSource: () => ({ buffer: null, connect: vi.fn(), start: vi.fn() }),
    resume: vi.fn(async () => {
      if (resumeError) throw resumeError;
      state = "running";
    }),
    close: vi.fn(async () => { state = "closed"; }),
  } as unknown as AudioContext;
  return { context, createBuffer };
}

const pcmPacket: BrowserAudioPacket = {
  sampleRate: 48_000,
  channels: 1,
  pcm: new DataView(new Uint8Array([0, 0, 1, 0]).buffer),
};

describe("browser PCM playback", () => {
  it("drops live packets without building latency while autoplay is suspended", () => {
    const states: BrowserAudioPlaybackState[] = [];
    const { context, createBuffer } = mockAudioContext();
    const player = new BrowserPcmPlayer((state) => states.push(state), () => context);

    expect(player.enqueue(pcmPacket)).toBe(false);
    expect(states).toEqual(["suspended"]);
    expect(createBuffer).not.toHaveBeenCalled();
  });

  it("resumes from a user action and schedules subsequent PCM", async () => {
    const states: BrowserAudioPlaybackState[] = [];
    const { context, createBuffer } = mockAudioContext();
    const player = new BrowserPcmPlayer((state) => states.push(state), () => context);

    expect(await player.resume()).toBe("running");
    expect(player.enqueue(pcmPacket)).toBe(true);
    expect(states).toContain("running");
    expect(createBuffer).toHaveBeenCalledOnce();
  });

  it("reports a rejected browser resume instead of failing silently", async () => {
    const states: BrowserAudioPlaybackState[] = [];
    const { context } = mockAudioContext(new Error("playback blocked"));
    const player = new BrowserPcmPlayer((state) => states.push(state), () => context);

    expect(await player.resume()).toBe("failed");
    expect(states.at(-1)).toBe("failed");
  });
});
