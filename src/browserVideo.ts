const packetMagic = [0x44, 0x48, 0x56, 0x31] as const;
const packetHeaderBytes = 28;
const hevcCodec = "hev1.1.6.L93.B0";
const decoderOutputTimeoutMs = 3_000;

export type BrowserVideoPacket = {
  key: boolean;
  timestamp: number;
  sequence: bigint;
  width: number;
  height: number;
  data: Uint8Array;
};

export function parseBrowserVideoPacket(buffer: ArrayBuffer): BrowserVideoPacket | null {
  if (buffer.byteLength < packetHeaderBytes) return null;
  const bytes = new Uint8Array(buffer);
  if (!packetMagic.every((value, index) => bytes[index] === value)) return null;
  const view = new DataView(buffer);
  const width = view.getUint16(24);
  const height = view.getUint16(26);
  if (width === 0 || height === 0) throw new Error("Browser video packet has invalid dimensions");
  return {
    key: view.getUint8(4) === 1,
    timestamp: Number(view.getBigUint64(8)),
    sequence: view.getBigUint64(16),
    width,
    height,
    data: new Uint8Array(buffer, packetHeaderBytes),
  };
}

type Callbacks = {
  output: (frame: VideoFrame, decodeMs: number) => void;
  requestKeyframe: () => void;
  fatal: (error: unknown) => void;
};

export class BrowserVideoDecoder {
  private decoder: VideoDecoder | null = null;
  private width = 0;
  private height = 0;
  private waitingForKeyframe = true;
  private failures = 0;
  private closed = false;
  private queue = Promise.resolve();
  private submitted = new Map<number, number>();
  private outputTimer: number | undefined;

  constructor(private readonly callbacks: Callbacks) {}

  enqueue(packet: BrowserVideoPacket) {
    this.queue = this.queue.then(() => this.decode(packet)).catch((error) => this.fail(error));
  }

  close() {
    this.closed = true;
    this.decoder?.close();
    this.decoder = null;
    this.submitted.clear();
    if (this.outputTimer !== undefined) window.clearTimeout(this.outputTimer);
    this.outputTimer = undefined;
  }

  private async configure(width: number, height: number) {
    if (this.decoder?.state === "configured" && width === this.width && height === this.height) return;
    if (this.decoder) {
      try {
        this.decoder.close();
      } catch {
        // A failed decoder may already be closed.
      }
      this.decoder = null;
    }
    const config: VideoDecoderConfig = {
      codec: hevcCodec,
      codedWidth: width,
      codedHeight: height,
      hardwareAcceleration: "prefer-hardware",
      optimizeForLatency: true,
    };
    if (!("VideoDecoder" in window)) throw new Error("WebCodecs VideoDecoder is unavailable");
    const support = await VideoDecoder.isConfigSupported(config);
    if (!support.supported || !support.config) {
      throw new Error("HEVC WebCodecs decoding is unsupported");
    }
    this.decoder = new VideoDecoder({
      output: (frame) => {
        const submittedAt = this.submitted.get(frame.timestamp);
        this.submitted.delete(frame.timestamp);
        this.scheduleOutputTimeout();
        this.failures = 0;
        this.callbacks.output(frame, submittedAt === undefined ? 0 : performance.now() - submittedAt);
      },
      error: (error) => this.recover(error),
    });
    this.decoder.configure(support.config);
    this.width = width;
    this.height = height;
    this.waitingForKeyframe = true;
  }

  private async decode(packet: BrowserVideoPacket) {
    if (this.closed) return;
    await this.configure(packet.width, packet.height);
    if (this.waitingForKeyframe && !packet.key) {
      this.callbacks.requestKeyframe();
      return;
    }
    if (!this.decoder || this.decoder.state !== "configured") return;
    if (this.decoder.decodeQueueSize > 8) {
      this.recover(new Error("Browser decoder queue exceeded its latency budget"));
      return;
    }
    this.waitingForKeyframe = false;
    this.submitted.set(packet.timestamp, performance.now());
    this.decoder.decode(new EncodedVideoChunk({
      type: packet.key ? "key" : "delta",
      timestamp: packet.timestamp,
      data: packet.data,
    }));
    this.scheduleOutputTimeout();
  }

  private scheduleOutputTimeout() {
    if (this.outputTimer !== undefined) window.clearTimeout(this.outputTimer);
    this.outputTimer = undefined;
    if (this.closed || this.submitted.size === 0) return;
    const oldestSubmission = Math.min(...this.submitted.values());
    const remaining = Math.max(0, decoderOutputTimeoutMs - (performance.now() - oldestSubmission));
    this.outputTimer = window.setTimeout(() => {
      this.outputTimer = undefined;
      if (document.visibilityState === "hidden") {
        this.submitted.clear();
        return;
      }
      this.recover(new Error("Browser decoder produced no video frame within its latency budget"));
    }, remaining);
  }

  private recover(error: unknown) {
    if (this.closed) return;
    this.failures += 1;
    this.waitingForKeyframe = true;
    this.submitted.clear();
    if (this.outputTimer !== undefined) window.clearTimeout(this.outputTimer);
    this.outputTimer = undefined;
    try {
      if (this.decoder?.state === "configured") this.decoder.reset();
      else this.decoder = null;
    } catch {
      try {
        this.decoder?.close();
      } catch {
        // A decoder error callback may have already moved it to `closed`.
      }
      this.decoder = null;
    }
    if (this.failures >= 3) this.fail(error);
    else this.callbacks.requestKeyframe();
  }

  private fail(error: unknown) {
    if (this.closed) return;
    this.close();
    this.callbacks.fatal(error);
  }
}
