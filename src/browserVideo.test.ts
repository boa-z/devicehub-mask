import { afterEach, describe, expect, it, vi } from "vitest";
import { BrowserVideoDecoder, hevcCodecFromAnnexB, parseBrowserVideoPacket, type BrowserVideoPacket } from "./browserVideo";

afterEach(() => vi.unstubAllGlobals());

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

describe("HEVC codec configuration", () => {
  it("derives profile, compatibility, tier, level, and constraints from SPS", () => {
    const sps = Uint8Array.from([
      0, 0, 0, 1, 0x42, 0x01,
      0x01, 0x01, 0x60, 0, 0, 0, 0xb0, 0, 0, 0, 0, 0, 153,
    ]);
    expect(hevcCodecFromAnnexB(sps)).toBe("hev1.1.6.L153.B0");
  });

  it("ignores non-SPS Annex-B units", () => {
    expect(hevcCodecFromAnnexB(Uint8Array.from([0, 0, 1, 0x26, 0x01]))).toBeNull();
  });
});

describe("browser video decoder recovery", () => {
  it("retries a simpler configuration when configure rejects a supported candidate", async () => {
    class InconsistentVideoDecoder {
      static instances: InconsistentVideoDecoder[] = [];
      static async isConfigSupported(config: VideoDecoderConfig) {
        return { supported: true, config };
      }

      state: CodecState = "unconfigured";
      decodeQueueSize = 0;
      decodeCalls = 0;

      constructor() {
        InconsistentVideoDecoder.instances.push(this);
      }

      configure(config: VideoDecoderConfig) {
        if (config.optimizeForLatency) throw new DOMException("Unsupported configuration", "OperationError");
        this.state = "configured";
      }

      decode() { this.decodeCalls += 1; }
      reset() { this.state = "unconfigured"; }
      close() { this.state = "closed"; }
    }

    vi.stubGlobal("VideoDecoder", InconsistentVideoDecoder);
    vi.stubGlobal("EncodedVideoChunk", class {});
    vi.stubGlobal("window", { VideoDecoder: InconsistentVideoDecoder, setTimeout, clearTimeout });
    vi.stubGlobal("document", { visibilityState: "visible" });
    const fatal = vi.fn();
    const decoder = new BrowserVideoDecoder({ output: vi.fn(), requestKeyframe: vi.fn(), fatal });
    decoder.enqueue({
      key: true,
      timestamp: 1,
      sequence: 1n,
      width: 1632,
      height: 2176,
      data: Uint8Array.from([
        0, 0, 0, 1, 0x42, 0x01,
        0x01, 0x01, 0x60, 0, 0, 0, 0xb0, 0, 0, 0, 0, 0, 153,
        0, 0, 0, 1, 0x26, 0x01,
      ]),
    });

    await vi.waitFor(() => expect(InconsistentVideoDecoder.instances[1]?.decodeCalls).toBe(1));
    expect(InconsistentVideoDecoder.instances).toHaveLength(2);
    expect(fatal).not.toHaveBeenCalled();
    decoder.close();
  });

  it("reconfigures a reset decoder when dimensions have not changed", async () => {
    class FakeVideoDecoder {
      static instances: FakeVideoDecoder[] = [];
      static async isConfigSupported(config: VideoDecoderConfig) {
        return { supported: true, config };
      }

      state: CodecState = "unconfigured";
      decodeQueueSize = 0;
      configureCalls = 0;
      decodeCalls = 0;

      constructor(public init: VideoDecoderInit) {
        FakeVideoDecoder.instances.push(this);
      }

      configure() {
        this.configureCalls += 1;
        this.state = "configured";
      }

      decode() {
        this.decodeCalls += 1;
      }

      reset() {
        this.state = "unconfigured";
      }

      close() {
        this.state = "closed";
      }
    }

    vi.stubGlobal("VideoDecoder", FakeVideoDecoder);
    vi.stubGlobal("EncodedVideoChunk", class {});
    vi.stubGlobal("window", { VideoDecoder: FakeVideoDecoder, setTimeout, clearTimeout });
    vi.stubGlobal("document", { visibilityState: "visible" });
    const requestKeyframe = vi.fn();
    const decoder = new BrowserVideoDecoder({ output: vi.fn(), requestKeyframe, fatal: vi.fn() });
    const packet: BrowserVideoPacket = {
      key: true,
      timestamp: 1,
      sequence: 1n,
      width: 1290,
      height: 2796,
      data: Uint8Array.from([
        0, 0, 0, 1, 0x42, 0x01,
        0x01, 0x01, 0x60, 0, 0, 0, 0xb0, 0, 0, 0, 0, 0, 153,
        0, 0, 0, 1, 0x26, 0x01,
      ]),
    };

    decoder.enqueue(packet);
    await vi.waitFor(() => expect(FakeVideoDecoder.instances[0]?.decodeCalls).toBe(1));
    FakeVideoDecoder.instances[0].init.error(new DOMException("decode failed"));
    decoder.enqueue({ ...packet, timestamp: 2, sequence: 2n });
    await vi.waitFor(() => expect(FakeVideoDecoder.instances).toHaveLength(2));

    expect(FakeVideoDecoder.instances[1].configureCalls).toBe(1);
    expect(FakeVideoDecoder.instances[1].decodeCalls).toBe(1);
    expect(requestKeyframe).toHaveBeenCalled();
    decoder.close();
  });
});
