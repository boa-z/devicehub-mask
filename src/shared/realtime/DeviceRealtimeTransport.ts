import type { BackendClient } from "../backend/client";

export type DeviceRealtimeClose = {
  event: CloseEvent;
  unexpected: boolean;
};

export type DeviceRealtimeHandlers = {
  onOpen?: () => void;
  onClose?: (close: DeviceRealtimeClose) => void;
  onText?: (data: string) => void;
  onBinary?: (data: ArrayBuffer) => void;
};

export type WebSocketFactory = (url: string, protocols: string[]) => WebSocket;

const defaultWebSocketFactory: WebSocketFactory = (url, protocols) => new WebSocket(url, protocols);

/** Owns the authenticated device websocket without knowing video, audio, or control messages. */
export class DeviceRealtimeTransport {
  private socket: WebSocket | null = null;

  private retryTimer: number | undefined;

  private disposed = false;

  private started = false;

  constructor(
    private readonly client: BackendClient,
    private readonly deviceId: string,
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
    if (this.retryTimer !== undefined) window.clearTimeout(this.retryTimer);
    this.retryTimer = undefined;
    this.socket?.close();
  }

  send(payload: unknown) {
    if (this.socket?.readyState !== WebSocket.OPEN) return false;
    this.socket.send(JSON.stringify(payload));
    return true;
  }

  private open() {
    if (!this.started || this.disposed) return;
    const socket = this.createSocket(
      this.client.websocketUrl("/api/ws", { device_id: this.deviceId }),
      this.client.websocketProtocols(),
    );
    this.socket = socket;
    let transportFailed = false;
    socket.binaryType = "arraybuffer";
    socket.onopen = () => this.handlers.onOpen?.();
    socket.onerror = () => {
      transportFailed = true;
    };
    socket.onclose = (event) => {
      const ownsCurrentSocket = this.socket === socket;
      const unexpected = !this.disposed && (transportFailed || !event.wasClean);
      if (ownsCurrentSocket) this.socket = null;
      this.handlers.onClose?.({ event, unexpected });
      if (this.started && !this.disposed) {
        this.retryTimer = window.setTimeout(() => {
          this.retryTimer = undefined;
          this.open();
        }, this.reconnectDelayMs);
      }
    };
    socket.onmessage = (event) => {
      if (typeof event.data === "string") {
        this.handlers.onText?.(event.data);
      } else if (event.data instanceof ArrayBuffer) {
        this.handlers.onBinary?.(event.data);
      }
    };
  }
}
