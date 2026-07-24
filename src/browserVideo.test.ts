import { describe, expect, it } from "vitest";
import { parseBrowserVideoPacket } from "./browserVideo";

describe("browser video packet", () => {
  it("parses the versioned big-endian header", () => {
    const buffer = new ArrayBuffer(31);
    const bytes = new Uint8Array(buffer);
    bytes.set([0x44, 0x48, 0x56, 0x31]);
    const view = new DataView(buffer);
    view.setUint8(4, 1);
    view.setBigUint64(8, 16_667n);
    view.setBigUint64(16, 7n);
    view.setUint16(24, 1290);
    view.setUint16(26, 2796);
    bytes.set([1, 2, 3], 28);

    const packet = parseBrowserVideoPacket(buffer);
    expect(packet).toMatchObject({ key: true, timestamp: 16_667, sequence: 7n, width: 1290, height: 2796 });
    expect([...packet!.data]).toEqual([1, 2, 3]);
  });

  it("leaves legacy JPEG messages untouched", () => {
    expect(parseBrowserVideoPacket(new Uint8Array([0xff, 0xd8, 0xff]).buffer)).toBeNull();
  });
});
