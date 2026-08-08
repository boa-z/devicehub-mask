import { useCallback, useEffect, useRef, useState } from "react";
import { BrowserVideoDecoder, BrowserVideoSequenceTracker, parseBrowserVideoPacket } from "./video/browserVideo";
import { BrowserPcmPlayer, parseBrowserAudioPacket, type BrowserAudioPlaybackState } from "./audio/browserAudio";
import { logFrontend } from "../../diagnostics";
import { DeviceRealtimeTransport } from "../../shared/realtime/DeviceRealtimeTransport";
import { hasSourceVideoActivity, isVideoStreamStalled } from "../../streamHealth";
import type { ClipboardEvent, DeviceEvent, DeviceStatus, Orientation, StreamMetrics } from "../../types";
import type { BackendClient } from "../../shared/backend/client";

const emptyMetrics: StreamMetrics = {
  transport_active: false,
  source_fps: 0,
  decoded_fps: 0,
  published_fps: 0,
  sent_fps: 0,
  backend_dropped_fps: 0,
  frame_age_ms: 0,
  websocket_send_ms: 0,
  decoder_accept_ms: 0,
  presentation_ack_ms: 0,
  megabits_per_second: 0,
};

type Options = {
  backend: BackendClient | null;
  deviceId: string | null;
  orientation: Orientation;
  videoDemand: boolean;
  monitorStall: boolean;
  audioEnabled: boolean;
  audioMuted: boolean;
  audioVolume: number;
  onStatus: (status: DeviceStatus) => void;
  onClipboard: (event: ClipboardEvent) => void;
  onDeviceEvent: (event: DeviceEvent) => void;
  onKeymapStatus: (status: KeymapStatus) => void;
  onDisconnect?: () => void;
};

export type KeymapStatus = {
  configured: boolean;
  active_mapping_ids: string[];
  unavailable_mapping_ids?: string[];
  active_contact_ids?: number[];
  active_contacts?: KeymapContact[];
  control_mode?: "mapping" | "keyboard" | null;
  error?: string;
};

export type KeymapContact = {
  identity: number;
  touching: boolean;
  x: number;
  y: number;
};

type FrontendMetrics = {
  startedAt: number;
  receivedFrames: number;
  replacedFrames: number;
  presentedFrames: number;
  decoderOutputMs: number;
  canvasDrawMs: number;
  decoderCongestions: number;
  decodeErrors: number;
};

function createFrontendMetrics(startedAt = performance.now()): FrontendMetrics {
  return {
    startedAt,
    receivedFrames: 0,
    replacedFrames: 0,
    presentedFrames: 0,
    decoderOutputMs: 0,
    canvasDrawMs: 0,
    decoderCongestions: 0,
    decodeErrors: 0,
  };
}

export function controlLeaseGrant(message: { type: string; payload?: unknown }): boolean | null {
  if (message.type !== "control_lease" || typeof message.payload !== "object" || message.payload === null) return null;
  const granted = (message.payload as { granted?: unknown }).granted;
  return typeof granted === "boolean" ? granted : null;
}

export function drawVideoFrame(
  canvas: HTMLCanvasElement,
  context: CanvasRenderingContext2D,
  source: CanvasImageSource,
  sourceWidth: number,
  sourceHeight: number,
  orientation: Orientation,
) {
  const landscape = orientation === "landscape_left" || orientation === "landscape_right";
  const width = landscape ? sourceHeight : sourceWidth;
  const height = landscape ? sourceWidth : sourceHeight;
  if (canvas.width !== width || canvas.height !== height) {
    canvas.width = width;
    canvas.height = height;
  }
  context.save();
  if (orientation === "landscape_right") {
    context.translate(canvas.width, 0);
    context.rotate(Math.PI / 2);
  } else if (orientation === "landscape_left") {
    context.translate(0, canvas.height);
    context.rotate(-Math.PI / 2);
  } else if (orientation === "portrait_upside_down") {
    context.translate(canvas.width, canvas.height);
    context.rotate(Math.PI);
  }
  context.drawImage(source, 0, 0);
  context.restore();
  return { width, height };
}

export function useDeviceVideoStream({
  backend,
  deviceId,
  orientation,
  videoDemand,
  monitorStall,
  audioEnabled,
  audioMuted,
  audioVolume,
  onStatus,
  onClipboard,
  onDeviceEvent,
  onKeymapStatus,
  onDisconnect,
}: Options) {
  const [connected, setConnected] = useState(false);
  const [controlGranted, setControlGranted] = useState(false);
  const [streamMetrics, setStreamMetrics] = useState<StreamMetrics>(emptyMetrics);
  const [renderFps, setRenderFps] = useState(0);
  const [frameSize, setFrameSize] = useState({ width: 1296, height: 2816 });
  const [hasFrame, setHasFrame] = useState(false);
  const [canvasReady, setCanvasReady] = useState(false);
  const [streamStalled, setStreamStalled] = useState(false);
  const [decoderError, setDecoderError] = useState<string | null>(null);
  const [browserAudioState, setBrowserAudioState] = useState<BrowserAudioPlaybackState>("idle");
  const canvasRef = useRef<HTMLCanvasElement>(null);
  const canvasContextRef = useRef<CanvasRenderingContext2D | null>(null);
  const canvasReadyRef = useRef(false);
  const hasFrameRef = useRef(false);
  const renderedFramesRef = useRef(0);
  const lastSourceActivityAtRef = useRef(0);
  const lastDecodedActivityAtRef = useRef(0);
  const transportRef = useRef<DeviceRealtimeTransport | null>(null);
  const orientationRef = useRef(orientation);
  const videoDemandRef = useRef(videoDemand);
  const callbacksRef = useRef({ onStatus, onClipboard, onDeviceEvent, onKeymapStatus, onDisconnect });
  const audioPreferencesRef = useRef({ enabled: audioEnabled, muted: audioMuted, volume: audioVolume });
  const audioPlayerRef = useRef<BrowserPcmPlayer | null>(null);
  orientationRef.current = orientation;
  videoDemandRef.current = videoDemand;
  callbacksRef.current = { onStatus, onClipboard, onDeviceEvent, onKeymapStatus, onDisconnect };
  audioPreferencesRef.current = { enabled: audioEnabled, muted: audioMuted, volume: audioVolume };

  useEffect(() => {
    audioPlayerRef.current?.setPreferences(audioMuted, audioVolume);
  }, [audioMuted, audioVolume]);

  const resumeBrowserAudio = useCallback(async () => {
    const player = audioPlayerRef.current;
    if (!player) return "idle" as const;
    return player.resume();
  }, []);

  useEffect(() => {
    const resume = () => {
      if (audioEnabled) void resumeBrowserAudio();
    };
    window.addEventListener("pointerdown", resume);
    window.addEventListener("keydown", resume);
    return () => {
      window.removeEventListener("pointerdown", resume);
      window.removeEventListener("keydown", resume);
    };
  }, [audioEnabled, resumeBrowserAudio]);

  const bindCanvas = useCallback((canvas: HTMLCanvasElement | null) => {
    canvasRef.current = canvas;
    canvasContextRef.current = null;
    if (canvas) {
      canvasReadyRef.current = false;
      setCanvasReady(false);
    }
  }, []);

  const send = useCallback((payload: unknown) => {
    transportRef.current?.send(payload);
  }, []);

  useEffect(() => {
    send({ type: "video_demand", active: videoDemand });
  }, [send, videoDemand]);

  useEffect(() => {
    send({ type: "audio_demand", active: audioEnabled && !audioMuted });
  }, [audioEnabled, audioMuted, send]);

  useEffect(() => {
    let measuredAt = performance.now();
    const timer = window.setInterval(() => {
      const now = performance.now();
      const elapsed = Math.max((now - measuredAt) / 1000, Number.EPSILON);
      setRenderFps(renderedFramesRef.current / elapsed);
      renderedFramesRef.current = 0;
      measuredAt = now;
    }, 1_000);
    return () => window.clearInterval(timer);
  }, []);

  useEffect(() => {
    if (!connected || !monitorStall) {
      setStreamStalled(false);
      return;
    }
    const update = () => {
      setStreamStalled(isVideoStreamStalled(
        performance.now(),
        lastSourceActivityAtRef.current,
        lastDecodedActivityAtRef.current,
      ));
    };
    update();
    const timer = window.setInterval(update, 1_000);
    return () => window.clearInterval(timer);
  }, [connected, monitorStall]);

  useEffect(() => {
    if (!backend || !deviceId) return;
    setHasFrame(false);
    setControlGranted(false);
    let receivedWebCodecsPacket = false;
    let browserSequence = new BrowserVideoSequenceTracker();
    let browserDecoder: BrowserVideoDecoder | null = null;
    let audioPlayer: BrowserPcmPlayer | null = null;
    let metricsTimer: number | undefined;
    let frontendMetrics = createFrontendMetrics();
    const presentFrame = (
      source: CanvasImageSource,
      sourceWidth: number,
      sourceHeight: number,
    ) => {
      const canvas = canvasRef.current;
      if (!canvas) return;
      const cachedContext = canvasContextRef.current;
      const context = cachedContext?.canvas === canvas
        ? cachedContext
        : canvas.getContext("2d", { alpha: false });
      if (!context) return;
      canvasContextRef.current = context;
      const drawStarted = performance.now();
      const size = drawVideoFrame(
        canvas,
        context,
        source,
        sourceWidth,
        sourceHeight,
        orientationRef.current,
      );
      frontendMetrics.canvasDrawMs += performance.now() - drawStarted;
      frontendMetrics.presentedFrames += 1;
      renderedFramesRef.current += 1;
      lastDecodedActivityAtRef.current = performance.now();
      setStreamStalled(false);
      setDecoderError(null);
      if (!canvasReadyRef.current) {
        canvasReadyRef.current = true;
        setCanvasReady(true);
      }
      if (!hasFrameRef.current) {
        hasFrameRef.current = true;
        setHasFrame(true);
      }
      setFrameSize((current) => current.width === size.width && current.height === size.height ? current : size);
    };
    const flushFrontendMetrics = () => {
      const now = performance.now();
      transport.send({
        type: "frontend_metrics",
        window_ms: now - frontendMetrics.startedAt,
        received_frames: frontendMetrics.receivedFrames,
        replaced_frames: frontendMetrics.replacedFrames,
        presented_frames: frontendMetrics.presentedFrames,
        decoder_output_ms: frontendMetrics.decoderOutputMs,
        canvas_draw_ms: frontendMetrics.canvasDrawMs,
        decoder_congestions: frontendMetrics.decoderCongestions,
        decode_errors: frontendMetrics.decodeErrors,
      });
      frontendMetrics = createFrontendMetrics(now);
    };
    const transport = new DeviceRealtimeTransport(backend, deviceId, {
      onOpen: () => {
        transportRef.current = transport;
        logFrontend("info", "websocket", "opened", "Video and control socket connected");
        receivedWebCodecsPacket = false;
        browserSequence = new BrowserVideoSequenceTracker();
        frontendMetrics = createFrontendMetrics();
        browserDecoder = new BrowserVideoDecoder({
          output: (frame, decodeMs, sequence) => {
            try {
              frontendMetrics.decoderOutputMs += decodeMs;
              presentFrame(frame, frame.codedWidth, frame.codedHeight);
            } finally {
              frame.close();
              transport.send({ type: "frame_presented", sequence: sequence.toString() });
            }
          },
          requestKeyframe: () => {
            transport.send({ type: "browser_video_keyframe" });
          },
          congestion: (decodeQueueSize) => {
            frontendMetrics.decoderCongestions += 1;
            logFrontend(
              "warn",
              "video",
              "browser_decoder_congestion",
              `WebCodecs queue saturated at ${decodeQueueSize} frames; resyncing from a keyframe`,
            );
          },
          fatal: (error) => {
            frontendMetrics.decodeErrors += 1;
            setDecoderError(String(error));
            logFrontend("warn", "video", "browser_decoder", error);
            transport.send({ type: "browser_decoder_error", message: String(error) });
          },
        });
        audioPlayer = new BrowserPcmPlayer((state, error) => {
          setBrowserAudioState(state);
          if (state === "suspended" || state === "failed") {
            const userActivation = (navigator as Navigator & {
              userActivation?: { isActive: boolean; hasBeenActive: boolean };
            }).userActivation;
            logFrontend(
              state === "failed" ? "warn" : "info",
              "audio",
              state === "failed" ? "browser_playback_failed" : "browser_playback_suspended",
              error ?? `origin=${location.origin} secure_context=${window.isSecureContext} user_active=${userActivation?.isActive ?? "unknown"} user_activated_before=${userActivation?.hasBeenActive ?? "unknown"}`,
            );
          } else if (state === "running") {
            logFrontend("info", "audio", "browser_playback_started", `origin=${location.origin}`);
          }
        });
        audioPlayer.setPreferences(audioPreferencesRef.current.muted, audioPreferencesRef.current.volume);
        audioPlayerRef.current = audioPlayer;
        const now = performance.now();
        lastSourceActivityAtRef.current = now;
        lastDecodedActivityAtRef.current = now;
        setConnected(true);
        transport.send({ type: "video_demand", active: videoDemandRef.current });
        const audio = audioPreferencesRef.current;
        transport.send({ type: "audio_demand", active: audio.enabled && !audio.muted });
        metricsTimer = window.setInterval(flushFrontendMetrics, 5_000);
      },
      onClose: ({ event, unexpected }) => {
        const ownsCurrentTransport = transportRef.current === transport;
        logFrontend(
          unexpected ? "warn" : "debug",
          "websocket",
          unexpected ? "transport_error" : "closed",
          `code=${event.code} clean=${event.wasClean} reason=${event.reason || "none"}`,
        );
        if (metricsTimer !== undefined) window.clearInterval(metricsTimer);
        metricsTimer = undefined;
        browserDecoder?.close();
        browserDecoder = null;
        audioPlayer?.close();
        if (audioPlayerRef.current === audioPlayer) audioPlayerRef.current = null;
        audioPlayer = null;
        if (ownsCurrentTransport) {
          callbacksRef.current.onKeymapStatus({ configured: false, active_mapping_ids: [], active_contacts: [] });
          setBrowserAudioState("idle");
          callbacksRef.current.onDisconnect?.();
          transportRef.current = null;
          setConnected(false);
          setControlGranted(false);
          lastSourceActivityAtRef.current = 0;
          lastDecodedActivityAtRef.current = 0;
          canvasReadyRef.current = false;
          setCanvasReady(false);
          setStreamStalled(false);
          setDecoderError(null);
          setStreamMetrics(emptyMetrics);
        }
      },
      onText: (data) => {
        const parsed = JSON.parse(data) as { type: string; payload: DeviceStatus | StreamMetrics | ClipboardEvent | DeviceEvent | KeymapStatus | { granted: boolean } };
        const leaseGrant = controlLeaseGrant(parsed);
        if (leaseGrant !== null) setControlGranted(leaseGrant);
        if (parsed.type === "status") callbacksRef.current.onStatus(parsed.payload as DeviceStatus);
        if (parsed.type === "clipboard") callbacksRef.current.onClipboard(parsed.payload as ClipboardEvent);
        if (parsed.type === "device_event") callbacksRef.current.onDeviceEvent(parsed.payload as DeviceEvent);
        if (parsed.type === "keymap_status") callbacksRef.current.onKeymapStatus(parsed.payload as KeymapStatus);
        if (parsed.type === "metrics") {
          const metrics = parsed.payload as StreamMetrics;
          setStreamMetrics(metrics);
          if (receivedWebCodecsPacket && metrics.decoded_fps > 0 && metrics.sent_fps === 0) {
            transport.send({ type: "browser_video_keyframe" });
          }
          if (hasSourceVideoActivity(metrics)) lastSourceActivityAtRef.current = performance.now();
        }
      },
      onBinary: (buffer) => {
        const currentAudioPlayer = audioPlayer;
        const currentBrowserDecoder = browserDecoder;
        if (!currentAudioPlayer || !currentBrowserDecoder) return;
        let audioPacket: ReturnType<typeof parseBrowserAudioPacket>;
        try {
          audioPacket = parseBrowserAudioPacket(buffer);
        } catch (error) {
          logFrontend("warn", "audio", "browser_packet", error);
          return;
        }
        if (audioPacket) {
          const preferences = audioPreferencesRef.current;
          currentAudioPlayer.setPreferences(preferences.muted, preferences.volume);
          if (preferences.enabled) currentAudioPlayer.enqueue(audioPacket);
          return;
        }
        frontendMetrics.receivedFrames += 1;
        lastSourceActivityAtRef.current = performance.now();
        let browserPacket: ReturnType<typeof parseBrowserVideoPacket>;
        try {
          browserPacket = parseBrowserVideoPacket(buffer);
        } catch (error) {
          frontendMetrics.decodeErrors += 1;
          logFrontend("warn", "video", "browser_packet", error);
          return;
        }
        if (browserPacket) {
          receivedWebCodecsPacket = true;
          const sequenceDecision = browserSequence.inspect(browserPacket);
          if (sequenceDecision.resetDecoder) currentBrowserDecoder.resync();
          if (!sequenceDecision.accept) {
            frontendMetrics.replacedFrames += 1;
            return;
          }
          const accepted = currentBrowserDecoder.enqueue(browserPacket);
          if (accepted) {
            transport.send({
              type: "browser_frame_accepted",
              sequence: browserPacket.sequence.toString(),
            });
          }
          return;
        }
        frontendMetrics.decodeErrors += 1;
        logFrontend("warn", "video", "unsupported_packet", "Received a non-WebCodecs video packet");
      },
    });
    transportRef.current = transport;
    transport.start();
    return () => transport.stop();
  }, [backend, deviceId]);

  return {
    connected,
    controlGranted,
    streamMetrics,
    renderFps,
    frameSize,
    hasFrame,
    canvasReady,
    streamStalled,
    decoderError,
    browserAudioState,
    resumeBrowserAudio,
    canvasRef,
    canvasReadyRef,
    bindCanvas,
    send,
    sendControl: send,
  };
}

export const useDeviceRealtimeSession = useDeviceVideoStream;
