import { afterEach, describe, expect, it, vi } from "vitest";
import { BrowserVideoDecoder, BrowserVideoSequenceTracker, browserVideoSequenceDiscontinuous, hevcCodecFromAnnexB, parseBrowserVideoPacket, type BrowserVideoPacket } from "./browserVideo";

afterEach(() => vi.unstubAllGlobals());

describe("browser video packet", () => {
  it("parses the versioned big-endian header", () => {
    const buffer = new ArrayBuffer(39);
    const bytes = new Uint8Array(buffer);
    bytes.set([0x44, 0x48, 0x56, 0x32]);
    const view = new DataView(buffer);
    view.setUint8(4, 1);
    view.setBigUint64(8, 16_667n);
    view.setBigUint64(16, 7n);
    view.setBigUint64(24, 3n);
    view.setUint16(32, 1290);
    view.setUint16(34, 2796);
    bytes.set([1, 2, 3], 36);

    const packet = parseBrowserVideoPacket(buffer);
    expect(packet).toMatchObject({ key: true, timestamp: 16_667, sequence: 7n, generation: 3n, width: 1290, height: 2796 });
    expect([...packet!.data]).toEqual([1, 2, 3]);
  });

  it("leaves legacy JPEG messages untouched", () => {
    expect(parseBrowserVideoPacket(new Uint8Array([0xff, 0xd8, 0xff]).buffer)).toBeNull();
  });

  it("requires resync for a delta-frame sequence gap but accepts a new keyframe", () => {
    const packet = {
      key: false,
      timestamp: 2,
      sequence: 9n,
      generation: 1n,
      width: 100,
      height: 200,
      data: new Uint8Array(),
    };
    expect(browserVideoSequenceDiscontinuous(7n, packet)).toBe(true);
    expect(browserVideoSequenceDiscontinuous(7n, { ...packet, key: true })).toBe(false);
    expect(browserVideoSequenceDiscontinuous(8n, packet)).toBe(false);
  });

  it("resets decoder state when a continuing socket receives a new stream generation", () => {
    const tracker = new BrowserVideoSequenceTracker();
    const packet: BrowserVideoPacket = {
      key: true,
      timestamp: 1,
      sequence: 10n,
      generation: 1n,
      width: 100,
      height: 200,
      data: new Uint8Array(),
    };

    expect(tracker.inspect(packet)).toEqual({ accept: true, resetDecoder: false });
    expect(tracker.inspect({ ...packet, sequence: 11n, key: false })).toEqual({ accept: true, resetDecoder: false });
    expect(tracker.inspect({ ...packet, generation: 2n, sequence: 12n })).toEqual({ accept: true, resetDecoder: true });
  });

  it("drops a new generation until its keyframe arrives", () => {
    const tracker = new BrowserVideoSequenceTracker();
    const packet: BrowserVideoPacket = {
      key: true,
      timestamp: 1,
      sequence: 1n,
      generation: 1n,
      width: 100,
      height: 200,
      data: new Uint8Array(),
    };

    tracker.inspect(packet);
    expect(tracker.inspect({ ...packet, generation: 2n, sequence: 2n, key: false })).toEqual({ accept: false, resetDecoder: true });
    expect(tracker.inspect({ ...packet, generation: 2n, sequence: 3n, key: false })).toEqual({ accept: false, resetDecoder: false });
    expect(tracker.inspect({ ...packet, generation: 2n, sequence: 4n })).toEqual({ accept: true, resetDecoder: false });
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
  it("bounds packets waiting for asynchronous decoder configuration", async () => {
    let resolveSupport: ((value: VideoDecoderSupport) => void) | undefined;
    class SlowVideoDecoder {
      static isConfigSupported(config: VideoDecoderConfig) {
        return new Promise<VideoDecoderSupport>((resolve) => {
          resolveSupport = () => resolve({ supported: true, config });
        });
      }
      state: CodecState = "unconfigured";
      decodeQueueSize = 0;
      constructor() {}
      configure() { this.state = "configured"; }
      decode() {}
      reset() { this.state = "unconfigured"; }
      close() { this.state = "closed"; }
    }

    vi.stubGlobal("VideoDecoder", SlowVideoDecoder);
    vi.stubGlobal("EncodedVideoChunk", class {});
    vi.stubGlobal("window", { VideoDecoder: SlowVideoDecoder, setTimeout, clearTimeout });
    vi.stubGlobal("document", { visibilityState: "visible" });
    const congestion = vi.fn();
    const requestKeyframe = vi.fn();
    const decoder = new BrowserVideoDecoder({ output: vi.fn(), requestKeyframe, congestion, fatal: vi.fn() });
    const packet: BrowserVideoPacket = {
      key: true,
      timestamp: 1,
      sequence: 1n,
      generation: 1n,
      width: 1290,
      height: 2796,
      data: Uint8Array.from([
        0, 0, 0, 1, 0x42, 0x01,
        0x01, 0x01, 0x60, 0, 0, 0, 0xb0, 0, 0, 0, 0, 0, 153,
      ]),
    };

    for (let index = 0; index < 8; index += 1) {
      expect(decoder.enqueue({ ...packet, timestamp: index + 1, sequence: BigInt(index + 1) })).toBe(true);
    }
    expect(decoder.enqueue({ ...packet, timestamp: 9, sequence: 9n })).toBe(false);
    expect(congestion).toHaveBeenCalledWith(8);
    expect(requestKeyframe).toHaveBeenCalledOnce();

    resolveSupport?.({ supported: false });
    decoder.close();
  });

  it("treats queue saturation as recoverable congestion instead of a fatal decoder failure", async () => {
    class SaturatedVideoDecoder {
      static instances: SaturatedVideoDecoder[] = [];
      static async isConfigSupported(config: VideoDecoderConfig) {
        return { supported: true, config };
      }

      state: CodecState = "unconfigured";
      decodeQueueSize = 9;

      constructor() {
        SaturatedVideoDecoder.instances.push(this);
      }

      configure() { this.state = "configured"; }
      decode() {}
      reset() { this.state = "unconfigured"; }
      close() { this.state = "closed"; }
    }

    vi.stubGlobal("VideoDecoder", SaturatedVideoDecoder);
    vi.stubGlobal("EncodedVideoChunk", class {});
    vi.stubGlobal("window", { VideoDecoder: SaturatedVideoDecoder, setTimeout, clearTimeout });
    vi.stubGlobal("document", { visibilityState: "visible" });
    const requestKeyframe = vi.fn();
    const congestion = vi.fn();
    const fatal = vi.fn();
    const decoder = new BrowserVideoDecoder({ output: vi.fn(), requestKeyframe, congestion, fatal });
    const packet: BrowserVideoPacket = {
      key: true,
      timestamp: 1,
      sequence: 1n,
      generation: 1n,
      width: 1290,
      height: 2796,
      data: Uint8Array.from([
        0, 0, 0, 1, 0x42, 0x01,
        0x01, 0x01, 0x60, 0, 0, 0, 0xb0, 0, 0, 0, 0, 0, 153,
        0, 0, 0, 1, 0x26, 0x01,
      ]),
    };

    for (let index = 0; index < 4; index += 1) {
      decoder.enqueue({ ...packet, timestamp: index + 1, sequence: BigInt(index + 1) });
      await vi.waitFor(() => expect(congestion).toHaveBeenCalledTimes(index + 1));
    }

    expect(requestKeyframe).toHaveBeenCalledTimes(4);
    expect(fatal).not.toHaveBeenCalled();
    decoder.close();
  });

  it("uses the reference hardware configuration before a generic fallback", async () => {
    class InconsistentVideoDecoder {
      static instances: InconsistentVideoDecoder[] = [];
      static configurations: VideoDecoderConfig[] = [];
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
        InconsistentVideoDecoder.configurations.push(config);
        if (config.hardwareAcceleration === "prefer-hardware") {
          throw new DOMException("Unsupported configuration", "OperationError");
        }
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
      generation: 1n,
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
    expect(InconsistentVideoDecoder.configurations[0]).toMatchObject({
      hardwareAcceleration: "prefer-hardware",
    });
    expect(InconsistentVideoDecoder.configurations[0]?.optimizeForLatency).toBeUndefined();
    expect(InconsistentVideoDecoder.configurations[1]).toMatchObject({
      hardwareAcceleration: "no-preference",
    });
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
      generation: 1n,
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
