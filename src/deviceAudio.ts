export type DeviceAudioPreferences = {
  muted: boolean;
  volume: number;
};

export const defaultDeviceAudioPreferences: DeviceAudioPreferences = {
  muted: false,
  volume: 0.8,
};

export type DeviceAudioControlAction = "unavailable" | "enable" | "unmute" | "resume" | "mute";

export function deviceAudioControlAction(
  enabled: boolean | null,
  muted: boolean,
  suspended: boolean,
): DeviceAudioControlAction {
  if (enabled === null) return "unavailable";
  if (!enabled) return "enable";
  if (muted) return "unmute";
  if (suspended) return "resume";
  return "mute";
}

export function shouldAttemptAudioResume(
  enabled: boolean | null,
  muted: boolean,
  running: boolean,
): boolean {
  return enabled === true && !muted && !running;
}

const storageKey = "devicehub-mask.device-audio";
const magic = [0x44, 0x48, 0x41, 0x50] as const;
const headerLength = 16;

export type PcmAudioChunk = {
  sampleRate: number;
  channels: number;
  frames: number;
  samples: Int16Array;
};

export function parseAudioEnvelope(buffer: ArrayBuffer): PcmAudioChunk | null {
  if (buffer.byteLength < headerLength) return null;
  const bytes = new Uint8Array(buffer);
  if (!magic.every((value, index) => bytes[index] === value)) return null;
  const view = new DataView(buffer);
  const version = bytes[4];
  const format = bytes[5];
  const channels = bytes[6];
  const sampleRate = view.getUint32(8, true);
  const frames = view.getUint32(12, true);
  const payloadBytes = frames * channels * 2;
  if (version !== 1 || format !== 1 || channels === 0 || channels > 8 || sampleRate === 0) {
    throw new Error("unsupported device audio envelope");
  }
  if (payloadBytes !== buffer.byteLength - headerLength) {
    throw new Error("invalid device audio payload length");
  }
  return {
    sampleRate,
    channels,
    frames,
    samples: new Int16Array(buffer, headerLength, frames * channels),
  };
}

export function parseDeviceAudioPreferences(value: string | null): DeviceAudioPreferences {
  if (value === null) return { ...defaultDeviceAudioPreferences };
  try {
    const parsed = JSON.parse(value) as Record<string, unknown>;
    return {
      muted: typeof parsed.muted === "boolean" ? parsed.muted : defaultDeviceAudioPreferences.muted,
      volume: typeof parsed.volume === "number" && Number.isFinite(parsed.volume)
        ? Math.min(1, Math.max(0, parsed.volume))
        : defaultDeviceAudioPreferences.volume,
    };
  } catch {
    return { ...defaultDeviceAudioPreferences };
  }
}

export function readDeviceAudioPreferences() {
  try {
    return parseDeviceAudioPreferences(localStorage.getItem(storageKey));
  } catch {
    return { ...defaultDeviceAudioPreferences };
  }
}

export function saveDeviceAudioPreferences(preferences: DeviceAudioPreferences) {
  try {
    localStorage.setItem(storageKey, JSON.stringify(preferences));
  } catch {
    // Preferences remain active for this session when storage is unavailable.
  }
}

export class PcmAudioPlayer {
  private context: AudioContext | null = null;
  private gain: GainNode | null = null;
  private nextStartTime = 0;
  private readonly sources = new Set<AudioBufferSourceNode>();
  private preferences: DeviceAudioPreferences;

  constructor(preferences: DeviceAudioPreferences) {
    this.preferences = preferences;
  }

  setPreferences(preferences: DeviceAudioPreferences) {
    this.preferences = preferences;
    if (this.gain && this.context) {
      this.gain.gain.setValueAtTime(preferences.muted ? 0 : preferences.volume, this.context.currentTime);
    }
  }

  async resume(): Promise<boolean> {
    const context = this.ensureContext();
    if (context.state !== "running") await context.resume();
    return context.state === "running";
  }

  isRunning(): boolean {
    return this.context?.state === "running";
  }

  isAudible(): boolean {
    return !this.preferences.muted && this.preferences.volume > 0;
  }

  push(chunk: PcmAudioChunk): boolean {
    const context = this.ensureContext();
    if (context.state !== "running" || !this.gain) return false;
    if (this.nextStartTime - context.currentTime > 0.25) this.reset();
    if (this.nextStartTime < context.currentTime) this.nextStartTime = context.currentTime + 0.06;

    const audioBuffer = context.createBuffer(chunk.channels, chunk.frames, chunk.sampleRate);
    for (let channel = 0; channel < chunk.channels; channel += 1) {
      const output = audioBuffer.getChannelData(channel);
      for (let frame = 0; frame < chunk.frames; frame += 1) {
        output[frame] = chunk.samples[frame * chunk.channels + channel] / 32768;
      }
    }
    const source = context.createBufferSource();
    source.buffer = audioBuffer;
    source.connect(this.gain);
    source.onended = () => this.sources.delete(source);
    this.sources.add(source);
    source.start(this.nextStartTime);
    this.nextStartTime += chunk.frames / chunk.sampleRate;
    return true;
  }

  reset() {
    for (const source of this.sources) {
      try { source.stop(); } catch { /* already stopped */ }
    }
    this.sources.clear();
    this.nextStartTime = 0;
  }

  close() {
    this.reset();
    void this.context?.close();
    this.context = null;
    this.gain = null;
  }

  private ensureContext() {
    if (!this.context) {
      this.context = new AudioContext({ latencyHint: "interactive", sampleRate: 48_000 });
      this.gain = this.context.createGain();
      this.gain.connect(this.context.destination);
      this.setPreferences(this.preferences);
    }
    return this.context;
  }
}
