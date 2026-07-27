import { describe, expect, it } from "vitest";
import { parseBrowserAudioPacket } from "./browserAudio";

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
