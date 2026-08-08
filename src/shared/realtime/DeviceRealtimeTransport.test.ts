import { describe, expect, it, vi } from "vitest";
import { BackendClient } from "../backend/client";
import { DeviceRealtimeTransport } from "./DeviceRealtimeTransport";

class FakeSocket {
  readyState = 0;
  binaryType: BinaryType = "blob";
  sent: string[] = [];
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

  close() {
    this.readyState = 3;
    this.onclose?.({ code: 1000, reason: "", wasClean: true } as CloseEvent);
  }

  text(payload: string) {
    this.onmessage?.({ data: payload } as MessageEvent);
  }
}

describe("device realtime transport", () => {
  it("opens an authenticated device socket and dispatches messages", () => {
    const socket = new FakeSocket();
    const onOpen = vi.fn();
    const onClose = vi.fn();
    const onText = vi.fn();
    const factory = vi.fn(() => socket as unknown as WebSocket);
    const client = new BackendClient({ origin: "http://127.0.0.1:54321", token: "session-token" });
    const transport = new DeviceRealtimeTransport(client, "phone::usb", { onOpen, onClose, onText }, factory);

    transport.start();
    expect(factory).toHaveBeenCalledWith(
      "ws://127.0.0.1:54321/api/ws?device_id=phone%3A%3Ausb",
      ["devicehub-mask", "session-token"],
    );
    socket.open();
    expect(socket.binaryType).toBe("arraybuffer");
    expect(onOpen).toHaveBeenCalledOnce();

    expect(transport.send({ type: "ping" })).toBe(true);
    expect(socket.sent).toEqual([JSON.stringify({ type: "ping" })]);
    socket.text(JSON.stringify({ type: "status" }));
    expect(onText).toHaveBeenCalledWith(JSON.stringify({ type: "status" }));

    transport.stop();
    expect(onClose).toHaveBeenCalledOnce();
    expect(onClose.mock.calls[0][0].unexpected).toBe(false);
  });
});
