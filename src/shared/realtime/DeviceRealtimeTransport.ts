import type { BackendClient } from "../backend/client";

export const REALTIME_PROTOCOL_VERSION = 2;

export type RealtimeChannel = "control" | "media";

export type ControlClientMessage =
  | { type: "multi_touch"; contacts: unknown[] }
  | { type: "button" | "button_down" | "button_up"; name: string }
  | { type: "system_action"; action: string }
  | { type: "keyboard_down" | "keyboard_up"; usage: number }
  | { type: "text"; text: string }
  | { type: "keymap_configure"; profile: unknown; frame: { width: number; height: number }; allow_scripts: boolean }
  | { type: "keymap_input"; keys: string[]; pointer_deltas: unknown[]; gamepad_axes: Record<string, number> }
  | { type: "keymap_direct_touches"; contacts: unknown[] }
  | { type: "keymap_debug"; enabled: boolean }
  | { type: "keymap_stop" }
  | { type: "rotate"; direction: "left" | "right" };

export type MediaClientMessage =
  | { type: "video_demand" | "audio_demand"; active: boolean }
  | { type: "browser_frame_accepted" | "frame_presented"; sequence: string }
  | { type: "browser_video_keyframe" }
  | { type: "browser_decoder_error"; message: string }
  | {
    type: "frontend_metrics";
    window_ms: number;
    received_frames: number;
    replaced_frames: number;
    presented_frames: number;
    decoder_output_ms: number;
    canvas_draw_ms: number;
    decoder_congestions: number;
    decode_errors: number;
  };

export type DeviceRealtimeClose = {
  event: CloseEvent;
  unexpected: boolean;
};

export type DeviceRealtimeHandlers = {
  onReady?: () => void;
  onClose?: (close: DeviceRealtimeClose) => void;
  onText?: (data: string) => void;
  onBinary?: (data: ArrayBuffer) => void;
};

export type WebSocketFactory = (url: string, protocols: string[]) => WebSocket;

const defaultWebSocketFactory: WebSocketFactory = (url, protocols) => new WebSocket(url, protocols);
const HANDSHAKE_TIMEOUT_MS = 5_000;

type ServerHello = {
  type: "server_hello";
  payload: {
    protocol_version: number;
    channel: RealtimeChannel;
  };
};

/** Owns one authenticated and negotiated realtime channel for a device. */
export class DeviceRealtimeTransport<TMessage extends ControlClientMessage | MediaClientMessage> {
  private socket: WebSocket | null = null;
  private retryTimer: number | undefined;
  private handshakeTimer: number | undefined;
  private disposed = false;
  private started = false;
  private ready = false;

  constructor(
    private readonly client: BackendClient,
    private readonly deviceId: string,
    private readonly channel: RealtimeChannel,
    private readonly handlers: DeviceRealtimeHandlers,
    private readonly createSocket: WebSocketFactory = defaultWebSocketFactory,
    private readonly reconnectDelayMs = 800,
  ) {}

  start() {
    if (this.started && !this.disposed) return;
    this.started = true;
    this.disposed = false;
    this.open();
  }

  stop() {
    this.started = false;
    this.disposed = true;
    this.ready = false;
    this.clearTimers();
    this.socket?.close();
  }

  send(payload: TMessage) {
    if (!this.ready || this.socket?.readyState !== WebSocket.OPEN) return false;
    this.socket.send(JSON.stringify(payload));
    return true;
  }

  private clearTimers() {
    if (this.retryTimer !== undefined) globalThis.clearTimeout(this.retryTimer);
    if (this.handshakeTimer !== undefined) globalThis.clearTimeout(this.handshakeTimer);
    this.retryTimer = undefined;
    this.handshakeTimer = undefined;
  }

  private open() {
    if (!this.started || this.disposed) return;
    const socket = this.createSocket(
      this.client.websocketUrl(`/api/ws/${this.channel}`, { device_id: this.deviceId }),
      this.client.websocketProtocols(),
    );
    this.socket = socket;
    this.ready = false;
    let transportFailed = false;
    socket.binaryType = "arraybuffer";
    socket.onopen = () => {
      socket.send(JSON.stringify({
        type: "client_hello",
        protocol_version: REALTIME_PROTOCOL_VERSION,
        channel: this.channel,
        platform: "web",
        client_version: "devicehub-mask-web",
        capabilities: this.channel === "media"
          ? ["hevc_webcodecs", "pcm_s16le"]
          : ["multi_touch", "hardware_buttons", "keyboard", "keymap"],
      }));
      this.handshakeTimer = globalThis.setTimeout(() => {
        if (this.socket === socket && !this.ready) socket.close(1002, "server_hello timed out");
      }, HANDSHAKE_TIMEOUT_MS);
    };
    socket.onerror = () => {
      transportFailed = true;
    };
    socket.onclose = (event) => {
      const ownsCurrentSocket = this.socket === socket;
      if (!ownsCurrentSocket) return;
      const unexpected = !this.disposed && (transportFailed || !event.wasClean || event.code !== 1000);
      this.socket = null;
      this.ready = false;
      if (this.handshakeTimer !== undefined) globalThis.clearTimeout(this.handshakeTimer);
      this.handshakeTimer = undefined;
      this.handlers.onClose?.({ event, unexpected });
      if (this.started && !this.disposed) {
        this.retryTimer = globalThis.setTimeout(() => {
          this.retryTimer = undefined;
          this.open();
        }, this.reconnectDelayMs);
      }
    };
    socket.onmessage = (event) => {
      if (!this.ready) {
        if (typeof event.data !== "string" || !this.acceptServerHello(event.data)) {
          socket.close(1002, "expected matching server_hello");
        }
        return;
      }
      if (typeof event.data === "string") {
        this.handlers.onText?.(event.data);
      } else if (event.data instanceof ArrayBuffer) {
        this.handlers.onBinary?.(event.data);
      }
    };
  }

  private acceptServerHello(data: string) {
    try {
      const hello = JSON.parse(data) as ServerHello;
      if (
        hello.type !== "server_hello"
        || hello.payload?.protocol_version !== REALTIME_PROTOCOL_VERSION
        || hello.payload?.channel !== this.channel
      ) return false;
      this.ready = true;
      if (this.handshakeTimer !== undefined) globalThis.clearTimeout(this.handshakeTimer);
      this.handshakeTimer = undefined;
      this.handlers.onReady?.();
      return true;
    } catch {
      return false;
    }
  }
}
