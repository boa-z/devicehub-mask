import { message } from "antd";
import { useCallback, useEffect, useRef, useState } from "react";
import type { TFunction } from "i18next";
import { logFrontend } from "../../diagnostics";
import { showErrorMessage } from "../../errorMessage";
import type { BackendClient } from "../../shared/backend/client";
import type { DeviceInventory, PairDeviceResult } from "../../types";
import { waitForDeviceSession } from "./deviceSelection";

const emptyInventory: DeviceInventory = { active_device_id: null, devices: [] };

type Options = {
  client: BackendClient | null;
  t: TFunction;
};

/** Owns manager-level discovery and lifecycle state independently from UI focus. */
export function useDeviceInventoryController({ client, t }: Options) {
  const [inventory, setInventory] = useState<DeviceInventory>(emptyInventory);
  const [pairingDeviceId, setPairingDeviceId] = useState<string | null>(null);
  const requestGenerationRef = useRef(0);

  const load = useCallback(async () => {
    if (!client) return null;
    const generation = ++requestGenerationRef.current;
    try {
      const response = await client.request("/api/devices");
      if (!response.ok) throw new Error(`${response.status} ${response.statusText}`);
      const next = await response.json() as DeviceInventory;
      if (requestGenerationRef.current === generation) setInventory(next);
      return next;
    } catch (error) {
      logFrontend("warn", "device_inventory", "refresh", error);
      return null;
    }
  }, [client]);

  useEffect(() => {
    if (!client) {
      requestGenerationRef.current += 1;
      setInventory(emptyInventory);
      return;
    }
    let disposed = false;
    let timer: number | undefined;
    const poll = async () => {
      await load();
      if (!disposed) {
        timer = globalThis.setTimeout(poll, document.visibilityState === "visible" ? 1_000 : 5_000);
      }
    };
    void poll();
    return () => {
      disposed = true;
      requestGenerationRef.current += 1;
      if (timer !== undefined) globalThis.clearTimeout(timer);
    };
  }, [client, load]);

  const refresh = useCallback(async () => {
    if (!client) return;
    try {
      const response = await client.request("/api/devices/refresh", { method: "PUT" });
      if (!response.ok) throw new Error(`${response.status} ${response.statusText}`);
      await load();
    } catch (error) {
      logFrontend("warn", "device", "refresh", error);
    }
  }, [client, load]);

  const connect = useCallback(async (deviceId: string) => {
    if (!client) return false;
    try {
      const response = await client.request(`/api/devices/${encodeURIComponent(deviceId)}/connect`, { method: "PUT" });
      if (!response.ok) throw new Error(`${response.status} ${response.statusText}`);
      const next = await waitForDeviceSession(client.request.bind(client), deviceId);
      setInventory(next);
      return true;
    } catch (error) {
      void showErrorMessage(t("errors.reconnectDevice", { error: String(error) }));
      return false;
    }
  }, [client, t]);

  const reconnect = useCallback(async (deviceId: string) => {
    if (!client) return false;
    try {
      const response = await client.request(`/api/devices/${encodeURIComponent(deviceId)}/reconnect`, { method: "PUT" });
      if (!response.ok) throw new Error(`${response.status} ${response.statusText}`);
      setInventory((current) => ({
        ...current,
        devices: current.devices.map((device) => device.id === deviceId
          ? { ...device, session_phase: "recovering", session_status: "reconnecting..." }
          : device),
      }));
      return true;
    } catch (error) {
      void showErrorMessage(t("errors.reconnectDevice", { error: String(error) }));
      return false;
    }
  }, [client, t]);

  const disconnect = useCallback(async (deviceId: string) => {
    if (!client) return false;
    try {
      const response = await client.request(`/api/devices/${encodeURIComponent(deviceId)}/connect`, { method: "DELETE" });
      if (!response.ok) throw new Error(`${response.status} ${response.statusText}`);
      setInventory((current) => ({
        ...current,
        active_device_id: current.active_device_id === deviceId ? null : current.active_device_id,
        devices: current.devices.map((device) => device.id === deviceId
          ? { ...device, session_phase: "disconnecting", session_status: "stopping..." }
          : device),
      }));
      return true;
    } catch (error) {
      void showErrorMessage(t("errors.disconnectDevice", { error: String(error) }));
      return false;
    }
  }, [client, t]);

  const pair = useCallback(async (deviceId: string) => {
    if (!client || pairingDeviceId) return false;
    const device = inventory.devices.find((candidate) => candidate.id === deviceId);
    if (!device || device.connection !== "USB" || device.pairing !== "unpaired") return false;
    const messageKey = "device-pairing";
    setPairingDeviceId(deviceId);
    void message.loading({ key: messageKey, content: t("device.pairingWaiting"), duration: 0 });
    try {
      const response = await client.request(`/api/devices/${encodeURIComponent(deviceId)}/pair`, { method: "PUT" });
      if (!response.ok) throw new Error(await response.text() || `${response.status} ${response.statusText}`);
      const result = await response.json() as PairDeviceResult;
      if (result.outcome === "paired") {
        void message.success({ key: messageKey, content: t("device.pairingSucceeded") });
        const next = await waitForDeviceSession(client.request.bind(client), deviceId);
        setInventory(next);
        return true;
      }
      const key = result.outcome === "denied"
        ? "device.pairingDenied"
        : result.outcome === "locked"
          ? "device.pairingLocked"
          : result.outcome === "timed_out"
            ? "device.pairingTimedOut"
            : "device.pairingFailed";
      void showErrorMessage(t(key, { error: result.error ?? t("device.pairingUnknownError") }), { key: messageKey });
      return false;
    } catch (error) {
      void showErrorMessage(t("device.pairingFailed", { error: String(error) }), { key: messageKey });
      return false;
    } finally {
      setPairingDeviceId(null);
    }
  }, [client, inventory.devices, pairingDeviceId, t]);

  return {
    inventory,
    pairingDeviceId,
    refresh,
    connect,
    reconnect,
    disconnect,
    pair,
  };
}
