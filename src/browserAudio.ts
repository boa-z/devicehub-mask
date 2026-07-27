const audioMagic = [0x44, 0x48, 0x41, 0x31] as const;
const audioHeaderBytes = 12;
const maxScheduledLatencySeconds = 0.25;
const initialScheduleLeadSeconds = 0.03;

export type BrowserAudioPacket = {
  sampleRate: number;
  channels: number;
  pcm: DataView;
};

export function parseBrowserAudioPacket(buffer: ArrayBuffer): BrowserAudioPacket | null {
  if (buffer.byteLength < audioHeaderBytes) return null;
  const bytes = new Uint8Array(buffer);
  if (!audioMagic.every((value, index) => bytes[index] === value)) return null;
  const view = new DataView(buffer);
  const sampleRate = view.getUint32(4);
  const channels = view.getUint16(8);
  const payloadBytes = buffer.byteLength - audioHeaderBytes;
  if (sampleRate < 8_000 || sampleRate > 192_000) throw new Error("Browser audio packet has an invalid sample rate");
  if (channels < 1 || channels > 8) throw new Error("Browser audio packet has an invalid channel count");
  if (payloadBytes === 0 || payloadBytes % (channels * 2) !== 0) {
    throw new Error("Browser audio packet has an invalid PCM payload length");
  }
  return {
    sampleRate,
    channels,
    pcm: new DataView(buffer, audioHeaderBytes),
  };
}

export class BrowserPcmPlayer {
  private context: AudioContext | null = null;
  private gain: GainNode | null = null;
  private scheduledUntil = 0;
  private muted = false;
  private volume = 0.8;

  setPreferences(muted: boolean, volume: number) {
    this.muted = muted;
    this.volume = Math.min(1, Math.max(0, volume));
    if (this.gain && this.context) {
      this.gain.gain.setValueAtTime(this.muted ? 0 : this.volume, this.context.currentTime);
    }
  }

  async resume() {
    const context = this.ensureContext(48_000);
    if (context.state === "suspended") await context.resume();
  }

  enqueue(packet: BrowserAudioPacket) {
    const context = this.ensureContext(packet.sampleRate);
    if (context.state !== "running") return false;
    const frames = packet.pcm.byteLength / 2 / packet.channels;
    const buffer = context.createBuffer(packet.channels, frames, packet.sampleRate);
    for (let channel = 0; channel < packet.channels; channel += 1) {
      const output = buffer.getChannelData(channel);
      for (let frame = 0; frame < frames; frame += 1) {
        const sample = packet.pcm.getInt16((frame * packet.channels + channel) * 2, true);
        output[frame] = sample < 0 ? sample / 32_768 : sample / 32_767;
      }
    }
    if (this.scheduledUntil > context.currentTime + maxScheduledLatencySeconds) {
      this.scheduledUntil = context.currentTime + initialScheduleLeadSeconds;
    }
    const startsAt = Math.max(this.scheduledUntil, context.currentTime + initialScheduleLeadSeconds);
    const source = context.createBufferSource();
    source.buffer = buffer;
    source.connect(this.gain!);
    source.start(startsAt);
    this.scheduledUntil = startsAt + buffer.duration;
    return true;
  }

  close() {
    const context = this.context;
    this.context = null;
    this.gain = null;
    this.scheduledUntil = 0;
    if (context) void context.close();
  }

  private ensureContext(sampleRate: number) {
    if (this.context) return this.context;
    const context = new AudioContext({ latencyHint: "interactive", sampleRate });
    const gain = context.createGain();
    gain.gain.value = this.muted ? 0 : this.volume;
    gain.connect(context.destination);
    this.context = context;
    this.gain = gain;
    return context;
  }
}
