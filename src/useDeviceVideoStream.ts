import { useCallback, useEffect, useRef, useState } from "react";
import { BrowserVideoDecoder, browserVideoSequenceDiscontinuous, parseBrowserVideoPacket } from "./browserVideo";
import { logFrontend } from "./diagnostics";
import { hasDecodedVideoActivity, hasSourceVideoActivity, isVideoStreamStalled } from "./streamHealth";
import type { ClipboardEvent, DeviceEvent, DeviceStatus, Orientation, StreamMetrics } from "./types";
import type { BackendConnection } from "./usePrivateBackend";

const emptyMetrics: StreamMetrics = {
  transport_active: false,
  source_fps: 0,
  decoded_fps: 0,
  published_fps: 0,
  sent_fps: 0,
  backend_dropped_fps: 0,
  jpeg_encode_ms: 0,
  frame_age_ms: 0,
  websocket_send_ms: 0,
  presentation_ack_ms: 0,
  megabits_per_second: 0,
};

type Options = {
  backend: BackendConnection | null;
  orientation: Orientation;
  videoDemand: boolean;
  monitorStall: boolean;
  onStatus: (status: DeviceStatus) => void;
  onClipboard: (event: ClipboardEvent) => void;
  onDeviceEvent: (event: DeviceEvent) => void;
  onDisconnect: () => void;
};

type FrontendMetrics = {
  startedAt: number;
  receivedFrames: number;
  replacedFrames: number;
  presentedFrames: number;
  jpegDecodeMs: number;
  canvasDrawMs: number;
  decodeErrors: number;
};

function createFrontendMetrics(startedAt = performance.now()): FrontendMetrics {
  return {
    startedAt,
    receivedFrames: 0,
    replacedFrames: 0,
    presentedFrames: 0,
    jpegDecodeMs: 0,
    canvasDrawMs: 0,
    decodeErrors: 0,
  };
}

function wsUrl(connection: BackendConnection) {
  return `${connection.origin.replace(/^http/, "ws")}/api/ws`;
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
  orientation,
  videoDemand,
  monitorStall,
  onStatus,
  onClipboard,
  onDeviceEvent,
  onDisconnect,
}: Options) {
  const [connected, setConnected] = useState(false);
  const [streamMetrics, setStreamMetrics] = useState<StreamMetrics>(emptyMetrics);
  const [renderFps, setRenderFps] = useState(0);
  const [frameSize, setFrameSize] = useState({ width: 1296, height: 2816 });
  const [hasFrame, setHasFrame] = useState(false);
  const [canvasReady, setCanvasReady] = useState(false);
  const [streamStalled, setStreamStalled] = useState(false);
  const canvasRef = useRef<HTMLCanvasElement>(null);
  const canvasContextRef = useRef<CanvasRenderingContext2D | null>(null);
  const canvasReadyRef = useRef(false);
  const hasFrameRef = useRef(false);
  const renderedFramesRef = useRef(0);
  const lastSourceActivityAtRef = useRef(0);
  const lastDecodedActivityAtRef = useRef(0);
  const socketRef = useRef<WebSocket | null>(null);
  const orientationRef = useRef(orientation);
  const videoDemandRef = useRef(videoDemand);
  const callbacksRef = useRef({ onStatus, onClipboard, onDeviceEvent, onDisconnect });
  orientationRef.current = orientation;
  videoDemandRef.current = videoDemand;
  callbacksRef.current = { onStatus, onClipboard, onDeviceEvent, onDisconnect };

  const bindCanvas = useCallback((canvas: HTMLCanvasElement | null) => {
    canvasRef.current = canvas;
    canvasContextRef.current = null;
    if (canvas) {
      canvasReadyRef.current = false;
      setCanvasReady(false);
    }
  }, []);

  const send = useCallback((payload: unknown) => {
    if (socketRef.current?.readyState === WebSocket.OPEN) {
      socketRef.current.send(JSON.stringify(payload));
    }
  }, []);

  useEffect(() => {
    send({ type: "video_demand", active: videoDemand });
  }, [send, videoDemand]);

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
    if (!backend) return;
    let disposed = false;
    let retry: number | undefined;
    let activeSocket: WebSocket | null = null;
    const open = () => {
      const socket = new WebSocket(wsUrl(backend), ["devicehub-mask", backend.token]);
      activeSocket = socket;
      let socketClosed = false;
      let pendingFrame: Blob | null = null;
      let decoding = false;
      let browserTransportActive = false;
      let lastBrowserSequence: bigint | null = null;
      let browserSequenceResync = false;
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
      const browserDecoder = new BrowserVideoDecoder({
        output: (frame, decodeMs) => {
          try {
            frontendMetrics.jpegDecodeMs += decodeMs;
            presentFrame(frame, frame.codedWidth, frame.codedHeight);
          } finally {
            frame.close();
          }
        },
        requestKeyframe: () => {
          if (socket.readyState === WebSocket.OPEN) {
            socket.send(JSON.stringify({ type: "browser_video_keyframe" }));
          }
        },
        fatal: (error) => {
          frontendMetrics.decodeErrors += 1;
          logFrontend("warn", "video", "browser_decoder", error);
          if (socket.readyState === WebSocket.OPEN) {
            socket.send(JSON.stringify({ type: "browser_decoder_error", message: String(error) }));
          }
        },
      });
      const flushFrontendMetrics = () => {
        const now = performance.now();
        if (socket.readyState === WebSocket.OPEN) {
          socket.send(JSON.stringify({
            type: "frontend_metrics",
            window_ms: now - frontendMetrics.startedAt,
            received_frames: frontendMetrics.receivedFrames,
            replaced_frames: frontendMetrics.replacedFrames,
            presented_frames: frontendMetrics.presentedFrames,
            jpeg_decode_ms: frontendMetrics.jpegDecodeMs,
            canvas_draw_ms: frontendMetrics.canvasDrawMs,
            decode_errors: frontendMetrics.decodeErrors,
          }));
        }
        frontendMetrics = createFrontendMetrics(now);
      };
      socket.binaryType = "arraybuffer";
      socket.onopen = () => {
        logFrontend("info", "websocket", "opened", "Video and control socket connected");
        socketRef.current = socket;
        const now = performance.now();
        lastSourceActivityAtRef.current = now;
        lastDecodedActivityAtRef.current = now;
        setConnected(true);
        socket.send(JSON.stringify({ type: "video_demand", active: videoDemandRef.current }));
        metricsTimer = window.setInterval(flushFrontendMetrics, 5_000);
      };
      socket.onerror = () => logFrontend("warn", "websocket", "transport_error", "WebSocket transport error");
      socket.onclose = (event) => {
        logFrontend(
          disposed ? "debug" : "warn",
          "websocket",
          "closed",
          `code=${event.code} clean=${event.wasClean} reason=${event.reason || "none"}`,
        );
        socketClosed = true;
        if (metricsTimer !== undefined) window.clearInterval(metricsTimer);
        pendingFrame = null;
        browserDecoder.close();
        callbacksRef.current.onDisconnect();
        if (socketRef.current === socket) socketRef.current = null;
        setConnected(false);
        lastSourceActivityAtRef.current = 0;
        lastDecodedActivityAtRef.current = 0;
        canvasReadyRef.current = false;
        setCanvasReady(false);
        setStreamStalled(false);
        setStreamMetrics(emptyMetrics);
        if (!disposed) retry = window.setTimeout(open, 800);
      };
      const drainFrames = async () => {
        if (decoding) return;
        decoding = true;
        try {
          while (pendingFrame && !disposed && !socketClosed) {
            const blob = pendingFrame;
            pendingFrame = null;
            let bitmap: ImageBitmap | null = null;
            try {
              const decodeStarted = performance.now();
              bitmap = await createImageBitmap(blob);
              frontendMetrics.jpegDecodeMs += performance.now() - decodeStarted;
              if (disposed || socketClosed) continue;
              presentFrame(bitmap, bitmap.width, bitmap.height);
            } catch (error) {
              frontendMetrics.decodeErrors += 1;
              logFrontend("warn", "video", "decode_frame", error);
            } finally {
              bitmap?.close();
              if (socket.readyState === WebSocket.OPEN) {
                socket.send(JSON.stringify({ type: "frame_presented" }));
              }
            }
          }
        } finally {
          decoding = false;
          if (pendingFrame && !disposed && !socketClosed) void drainFrames();
        }
      };
      socket.onmessage = (event) => {
        if (typeof event.data === "string") {
          const data = JSON.parse(event.data) as { type: string; payload: DeviceStatus | StreamMetrics | ClipboardEvent | DeviceEvent };
          if (data.type === "status") callbacksRef.current.onStatus(data.payload as DeviceStatus);
          if (data.type === "clipboard") callbacksRef.current.onClipboard(data.payload as ClipboardEvent);
          if (data.type === "device_event") callbacksRef.current.onDeviceEvent(data.payload as DeviceEvent);
          if (data.type === "metrics") {
            const metrics = data.payload as StreamMetrics;
            setStreamMetrics(metrics);
            if (browserTransportActive && metrics.decoded_fps > 0 && metrics.sent_fps === 0 && socket.readyState === WebSocket.OPEN) {
              socket.send(JSON.stringify({ type: "browser_video_keyframe" }));
            }
            if (hasSourceVideoActivity(metrics)) {
              lastSourceActivityAtRef.current = performance.now();
            }
            // In browser mode the backend decoded counter means an HEVC access
            // unit was forwarded, not that WebCodecs produced a VideoFrame.
            if (!browserTransportActive && hasDecodedVideoActivity(metrics)) {
              lastDecodedActivityAtRef.current = performance.now();
              setStreamStalled(false);
            }
          }
          return;
        }
        const buffer = event.data as ArrayBuffer;
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
          browserTransportActive = true;
          if (browserSequenceResync && !browserPacket.key) return;
          if (browserVideoSequenceDiscontinuous(lastBrowserSequence, browserPacket)) {
            frontendMetrics.replacedFrames += 1;
            browserSequenceResync = true;
            browserDecoder.resync();
            return;
          }
          if (browserPacket.key) browserSequenceResync = false;
          lastBrowserSequence = browserPacket.sequence;
          browserDecoder.enqueue(browserPacket);
          return;
        }
        browserTransportActive = false;
        lastBrowserSequence = null;
        browserSequenceResync = false;
        if (pendingFrame) {
          frontendMetrics.replacedFrames += 1;
          if (socket.readyState === WebSocket.OPEN) {
            socket.send(JSON.stringify({ type: "frame_presented" }));
          }
        }
        pendingFrame = new Blob([buffer], { type: "image/jpeg" });
        void drainFrames();
      };
    };
    open();
    return () => {
      disposed = true;
      if (retry !== undefined) window.clearTimeout(retry);
      activeSocket?.close();
    };
  }, [backend]);

  return {
    connected,
    streamMetrics,
    renderFps,
    frameSize,
    hasFrame,
    canvasReady,
    streamStalled,
    canvasRef,
    canvasReadyRef,
    bindCanvas,
    send,
  };
}
