import { message } from "antd";
import { useCallback, useEffect, useRef, useState } from "react";
import type { TFunction } from "i18next";
import { showErrorMessage } from "../../errorMessage";
import { logFrontend } from "../../diagnostics";
import { waitForDeviceSession } from "./deviceSelection";
import type { BackendClient } from "../../shared/backend/client";
import type { DeviceStatus, PairDeviceResult } from "../../types";

export const emptyDeviceStatus: DeviceStatus = {
  status: "",
  phase: "disconnected",
  updated_at_ms: 0,
  active_udid: null,
  active_device_id: null,
  error: null,
  orientation: "portrait",
  devices: [],
  location: { available: false, active: false, backend: null, latitude: null, longitude: null, error: null },
};

type Options = {
  client: BackendClient | null;
  startingStatus: string;
  onReleaseControls: () => void;
  t: TFunction;
};

export function useDeviceSessionController({ client, startingStatus, onReleaseControls, t }: Options) {
  const [status, setStatus] = useState<DeviceStatus>(() => ({ ...emptyDeviceStatus, status: startingStatus }));
  const [selectedDeviceId, setSelectedDeviceId] = useState<string | null>(null);
  const [pairingDeviceId, setPairingDeviceId] = useState<string | null>(null);
  const selectedDeviceIntentRef = useRef<string | null>(null);
  const releaseControlsRef = useRef(onReleaseControls);
  releaseControlsRef.current = onReleaseControls;

  useEffect(() => {
    const intended = selectedDeviceIntentRef.current;
    if (intended) {
      if (status.active_device_id === intended) {
        selectedDeviceIntentRef.current = null;
        setSelectedDeviceId(intended);
      } else if (status.devices.some((device) => device.id === intended)) {
        return;
      } else {
        selectedDeviceIntentRef.current = null;
      }
    }
    if (status.active_device_id) setSelectedDeviceId(status.active_device_id);
  }, [status.active_device_id, status.devices]);

  useEffect(() => {
    if (!client || selectedDeviceId) return;
    let disposed = false;
    const refreshStatus = async () => {
      try {
        const response = await client.request("/api/status");
        if (!response.ok) throw new Error(`${response.status} ${response.statusText}`);
        const next = await response.json() as DeviceStatus;
        if (!disposed) setStatus(next);
      } catch (error) {
        if (!disposed) logFrontend("warn", "backend", "initial_status", error);
      }
    };
    void refreshStatus();
    const timer = window.setInterval(() => void refreshStatus(), 500);
    return () => {
      disposed = true;
      window.clearInterval(timer);
    };
  }, [client, selectedDeviceId]);

  const refresh = useCallback(async () => {
    if (!client) return;
    try {
      const response = await client.request("/api/devices/refresh", { method: "PUT" });
      if (!response.ok) throw new Error(`${response.status} ${response.statusText}`);
    } catch (error) {
      logFrontend("warn", "device", "refresh", error);
    }
  }, [client]);

  const connect = useCallback(async (deviceId: string) => {
    if (!client) return;
    selectedDeviceIntentRef.current = deviceId;
    releaseControlsRef.current();
    try {
      const response = await client.request(`/api/devices/${encodeURIComponent(deviceId)}/connect`, { method: "PUT" });
      if (!response.ok) throw new Error(`${response.status} ${response.statusText}`);
      const next = await waitForDeviceSession(client.request.bind(client), deviceId);
      setStatus(next);
      setSelectedDeviceId(deviceId);
    } catch (error) {
      selectedDeviceIntentRef.current = null;
      void showErrorMessage(t("errors.reconnectDevice", { error: String(error) }));
    }
  }, [client, t]);

  const reconnect = useCallback(async (deviceId = selectedDeviceId) => {
    if (!client || !deviceId) return false;
    if (deviceId === selectedDeviceId) releaseControlsRef.current();
    try {
      const response = await client.request(`/api/devices/${encodeURIComponent(deviceId)}/reconnect`, { method: "PUT" });
      if (!response.ok) throw new Error(`${response.status} ${response.statusText}`);
      return true;
    } catch (error) {
      void showErrorMessage(t("errors.reconnectDevice", { error: String(error) }));
      return false;
    }
  }, [client, selectedDeviceId, t]);

  const select = useCallback(async (deviceId: string) => {
    const device = status.devices.find((candidate) => candidate.id === deviceId);
    if (device?.pairing === "unpaired") return;
    await connect(deviceId);
  }, [connect, status.devices]);

  const disconnect = useCallback(async (deviceId: string) => {
    const isSelected = deviceId === selectedDeviceId;
    if (isSelected) releaseControlsRef.current();
    if (!client) return;
    try {
      const response = await client.request(`/api/devices/${encodeURIComponent(deviceId)}/connect`, { method: "DELETE" });
      if (!response.ok) throw new Error(`${response.status} ${response.statusText}`);
      if (isSelected) {
        selectedDeviceIntentRef.current = null;
        setSelectedDeviceId(null);
        setStatus((current) => ({
          ...current,
          active_udid: null,
          active_device_id: null,
          devices: current.devices.map((device) => device.id === deviceId
            ? { ...device, session_phase: "disconnecting", session_status: "stopping..." }
            : device),
        }));
      }
    } catch (error) {
      void showErrorMessage(t("errors.disconnectDevice", { error: String(error) }));
    }
  }, [client, selectedDeviceId, t]);

  const pair = useCallback(async (deviceId: string) => {
    if (!client || pairingDeviceId) return;
    const device = status.devices.find((candidate) => candidate.id === deviceId);
    if (!device || device.connection !== "USB" || device.pairing !== "unpaired") return;
    const messageKey = "device-pairing";
    setPairingDeviceId(deviceId);
    void message.loading({ key: messageKey, content: t("device.pairingWaiting"), duration: 0 });
    try {
      const response = await client.request(`/api/devices/${encodeURIComponent(deviceId)}/pair`, { method: "PUT" });
      if (!response.ok) throw new Error(await response.text() || `${response.status} ${response.statusText}`);
      const result = await response.json() as PairDeviceResult;
      if (result.outcome === "paired") {
        void message.success({ key: messageKey, content: t("device.pairingSucceeded") });
        selectedDeviceIntentRef.current = deviceId;
        const next = await waitForDeviceSession(client.request.bind(client), deviceId);
        setStatus(next);
        setSelectedDeviceId(deviceId);
      } else {
        const key = result.outcome === "denied"
          ? "device.pairingDenied"
          : result.outcome === "locked"
            ? "device.pairingLocked"
            : result.outcome === "timed_out"
              ? "device.pairingTimedOut"
              : "device.pairingFailed";
        void showErrorMessage(t(key, { error: result.error ?? t("device.pairingUnknownError") }), { key: messageKey });
      }
    } catch (error) {
      void showErrorMessage(t("device.pairingFailed", { error: String(error) }), { key: messageKey });
    } finally {
      setPairingDeviceId(null);
    }
  }, [client, pairingDeviceId, status.devices, t]);

  return {
    status,
    setStatus,
    selectedDeviceId,
    pairingDeviceId,
    connect,
    disconnect,
    reconnect,
    select,
    pair,
    refresh,
  };
}
