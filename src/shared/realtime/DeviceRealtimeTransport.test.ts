import { describe, expect, it, vi } from "vitest";
import { BackendClient } from "../backend/client";
import {
  DeviceRealtimeTransport,
  REALTIME_PROTOCOL_VERSION,
  type MediaClientMessage,
} from "./DeviceRealtimeTransport";

class FakeSocket {
  readyState = 0;
  binaryType: BinaryType = "blob";
  sent: string[] = [];
  closeCode: number | undefined;
  onopen: ((event: Event) => void) | null = null;
  onclose: ((event: CloseEvent) => void) | null = null;
  onerror: ((event: Event) => void) | null = null;
  onmessage: ((event: MessageEvent) => void) | null = null;

  open() {
    this.readyState = WebSocket.OPEN;
    this.onopen?.(new Event("open"));
  }

  send(payload: string) {
    this.sent.push(payload);
  }

  close(code = 1000, reason = "") {
    this.closeCode = code;
    this.readyState = WebSocket.CLOSED;
    this.onclose?.({ code, reason, wasClean: code === 1000 } as CloseEvent);
  }

  text(payload: string) {
    this.onmessage?.({ data: payload } as MessageEvent);
  }
}

function serverHello(channel: "control" | "media") {
  return JSON.stringify({
    type: "server_hello",
    payload: { protocol_version: REALTIME_PROTOCOL_VERSION, channel },
  });
}

describe("device realtime transport", () => {
  it("becomes ready only after a matching media handshake", () => {
    const socket = new FakeSocket();
    const onReady = vi.fn();
    const onClose = vi.fn();
    const onText = vi.fn();
    const factory = vi.fn(() => socket as unknown as WebSocket);
    const client = new BackendClient({ origin: "http://127.0.0.1:54321", token: "session-token" });
    const transport = new DeviceRealtimeTransport<MediaClientMessage>(
      client,
      "phone::usb",
      "media",
      { onReady, onClose, onText },
      factory,
    );

    transport.start();
    expect(factory).toHaveBeenCalledWith(
      "ws://127.0.0.1:54321/api/ws/media?device_id=phone%3A%3Ausb",
      ["devicehub-mask", "session-token"],
    );
    socket.open();
    expect(socket.binaryType).toBe("arraybuffer");
    expect(onReady).not.toHaveBeenCalled();
    expect(transport.send({ type: "video_demand", active: true })).toBe(false);
    expect(JSON.parse(socket.sent[0])).toMatchObject({
      type: "client_hello",
      protocol_version: REALTIME_PROTOCOL_VERSION,
      channel: "media",
    });

    socket.text(serverHello("media"));
    expect(onReady).toHaveBeenCalledOnce();
    expect(transport.send({ type: "video_demand", active: true })).toBe(true);
    expect(JSON.parse(socket.sent[1])).toEqual({ type: "video_demand", active: true });
    socket.text(JSON.stringify({ type: "metrics" }));
    expect(onText).toHaveBeenCalledWith(JSON.stringify({ type: "metrics" }));

    transport.stop();
    expect(onClose).toHaveBeenCalledOnce();
    expect(onClose.mock.calls[0][0].unexpected).toBe(false);
  });

  it("rejects a server hello for the other channel", () => {
    const socket = new FakeSocket();
    const transport = new DeviceRealtimeTransport<MediaClientMessage>(
      new BackendClient({ origin: "http://127.0.0.1:54321", token: "session-token" }),
      "phone::usb",
      "media",
      {},
      () => socket as unknown as WebSocket,
    );

    transport.start();
    socket.open();
    socket.text(serverHello("control"));

    expect(socket.closeCode).toBe(1002);
  });
});
