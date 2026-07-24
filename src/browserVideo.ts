const packetMagic = [0x44, 0x48, 0x56, 0x31] as const;
const packetHeaderBytes = 28;
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

export function browserVideoSequenceDiscontinuous(previous: bigint | null, packet: BrowserVideoPacket): boolean {
  return previous !== null && packet.sequence !== previous + 1n && !packet.key;
}

export function hevcCodecFromAnnexB(data: Uint8Array): string | null {
  for (const nal of annexBNalUnits(data)) {
    if (nal.length < 15 || ((nal[0] >> 1) & 0x3f) !== 33) continue;
    const rbsp = removeEmulationPrevention(nal.subarray(2));
    if (rbsp.length < 13) continue;
    const profileByte = rbsp[1];
    const profileSpace = profileByte >> 6;
    const tier = (profileByte & 0x20) === 0 ? "L" : "H";
    const profileIdc = profileByte & 0x1f;
    const compatibility = reverseBits32(
      ((rbsp[2] << 24) | (rbsp[3] << 16) | (rbsp[4] << 8) | rbsp[5]) >>> 0,
    );
    const constraints = [...rbsp.subarray(6, 12)];
    while (constraints.at(-1) === 0) constraints.pop();
    const profilePrefix = profileSpace === 0 ? "" : String.fromCharCode(64 + profileSpace);
    const constraintSuffix = constraints.length === 0
      ? ""
      : `.${constraints.map((byte) => byte.toString(16).toUpperCase().padStart(2, "0")).join(".")}`;
    return `hev1.${profilePrefix}${profileIdc}.${compatibility}.${tier}${rbsp[12]}${constraintSuffix}`;
  }
  return null;
}

function annexBNalUnits(data: Uint8Array): Uint8Array[] {
  const starts: Array<{ offset: number; payload: number }> = [];
  for (let index = 0; index + 3 <= data.length;) {
    const length = index + 4 <= data.length
      && data[index] === 0 && data[index + 1] === 0 && data[index + 2] === 0 && data[index + 3] === 1
      ? 4
      : data[index] === 0 && data[index + 1] === 0 && data[index + 2] === 1
        ? 3
        : 0;
    if (length === 0) {
      index += 1;
      continue;
    }
    starts.push({ offset: index, payload: index + length });
    index += length;
  }
  return starts
    .map((start, index) => data.subarray(start.payload, starts[index + 1]?.offset ?? data.length))
    .filter((nal) => nal.length > 0);
}

function removeEmulationPrevention(data: Uint8Array): Uint8Array {
  const bytes: number[] = [];
  for (let index = 0; index < data.length; index += 1) {
    if (index >= 2 && data[index] === 3 && data[index - 1] === 0 && data[index - 2] === 0) continue;
    bytes.push(data[index]);
  }
  return Uint8Array.from(bytes);
}

function reverseBits32(value: number): number {
  let result = 0;
  for (let bit = 0; bit < 32; bit += 1) {
    result = (result * 2) + ((value >>> bit) & 1);
  }
  return result >>> 0;
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
  private codec: string | null = null;
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

  resync() {
    if (this.closed) return;
    this.waitingForKeyframe = true;
    this.submitted.clear();
    if (this.outputTimer !== undefined) window.clearTimeout(this.outputTimer);
    this.outputTimer = undefined;
    try {
      if (this.decoder?.state === "configured") this.decoder.reset();
      else this.decoder = null;
    } catch {
      try { this.decoder?.close(); } catch { /* The decoder may already be closed. */ }
      this.decoder = null;
    }
    this.callbacks.requestKeyframe();
  }

  private async configure(width: number, height: number, codec: string) {
    if (this.decoder?.state === "configured" && width === this.width && height === this.height && codec === this.codec) return;
    if (this.decoder) {
      try {
        this.decoder.close();
      } catch {
        // A failed decoder may already be closed.
      }
      this.decoder = null;
    }
    if (!("VideoDecoder" in window)) throw new Error("WebCodecs VideoDecoder is unavailable");
    const failures: string[] = [];
    for (const candidate of decoderCandidates(codec, width, height)) {
      let decoder: VideoDecoder | null = null;
      try {
        const support = await VideoDecoder.isConfigSupported(candidate);
        if (!support.supported) continue;
        decoder = new VideoDecoder({
          output: (frame) => {
            const submittedAt = this.submitted.get(frame.timestamp);
            this.submitted.delete(frame.timestamp);
            this.scheduleOutputTimeout();
            this.failures = 0;
            this.callbacks.output(frame, submittedAt === undefined ? 0 : performance.now() - submittedAt);
          },
          error: (error) => this.recover(error),
        });
        decoder.configure(support.config ?? candidate);
        this.decoder = decoder;
        break;
      } catch (error) {
        failures.push(String(error));
        try { decoder?.close(); } catch { /* The rejected decoder may already be closed. */ }
      }
    }
    if (!this.decoder) {
      const reason = failures.at(-1) ?? "no candidate configuration was reported as supported";
      throw new Error(`HEVC WebCodecs configuration failed (${codec}, ${width}x${height}): ${reason}`);
    }
    this.width = width;
    this.height = height;
    this.codec = codec;
    this.waitingForKeyframe = true;
  }

  private async decode(packet: BrowserVideoPacket) {
    if (this.closed) return;
    const configurationChanged = packet.width !== this.width || packet.height !== this.height;
    const codec = (!this.codec || packet.key || configurationChanged
      ? hevcCodecFromAnnexB(packet.data)
      : null) ?? this.codec;
    if (!codec) {
      this.callbacks.requestKeyframe();
      return;
    }
    await this.configure(packet.width, packet.height, codec);
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

function decoderCandidates(codec: string, width: number, height: number): VideoDecoderConfig[] {
  const codecs = [codec, codec.replace(/^hev1\./, "hvc1.")];
  return codecs.flatMap((candidateCodec) => [
    {
      codec: candidateCodec,
      codedWidth: width,
      codedHeight: height,
      hardwareAcceleration: "prefer-hardware" as HardwareAcceleration,
      optimizeForLatency: true,
    },
    {
      codec: candidateCodec,
      codedWidth: width,
      codedHeight: height,
      hardwareAcceleration: "no-preference" as HardwareAcceleration,
    },
    { codec: candidateCodec, codedWidth: width, codedHeight: height },
  ]);
}
