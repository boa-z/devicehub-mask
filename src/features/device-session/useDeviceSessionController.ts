import { useCallback, useEffect, useMemo, useRef, useState, type SetStateAction } from "react";
import type { TFunction } from "i18next";
import type { BackendClient } from "../../shared/backend/client";
import type { DeviceStatus } from "../../types";
import { isActiveSession } from "./deviceConnections";
import { useDeviceInventoryController } from "./useDeviceInventoryController";

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

/** Combines manager inventory with one UI-focused device session without merging their ownership. */
export function useDeviceSessionController({ client, startingStatus, onReleaseControls, t }: Options) {
  const [sessionStatus, setSessionStatus] = useState<DeviceStatus>(() => ({
    ...emptyDeviceStatus,
    status: startingStatus,
  }));
  const [selectedDeviceId, setSelectedDeviceId] = useState<string | null>(null);
  const selectedDeviceIntentRef = useRef<string | null>(null);
  const releaseControlsRef = useRef(onReleaseControls);
  releaseControlsRef.current = onReleaseControls;
  const {
    inventory,
    pairingDeviceId,
    refresh,
    connect: connectInventory,
    reconnect: reconnectInventory,
    disconnect: disconnectInventory,
    pair: pairInventory,
  } = useDeviceInventoryController({ client, t });

  useEffect(() => {
    if (selectedDeviceIntentRef.current || selectedDeviceId || !inventory.active_device_id) return;
    setSelectedDeviceId(inventory.active_device_id);
  }, [inventory.active_device_id, selectedDeviceId]);

  useEffect(() => {
    if (selectedDeviceId && !inventory.devices.some((device) => device.id === selectedDeviceId)) {
      setSelectedDeviceId(null);
    }
  }, [inventory.devices, selectedDeviceId]);

  const status = useMemo<DeviceStatus>(() => ({
    ...sessionStatus,
    active_device_id: selectedDeviceId,
    active_udid: selectedDeviceId
      ? inventory.devices.find((device) => device.id === selectedDeviceId)?.udid ?? sessionStatus.active_udid
      : null,
    devices: inventory.devices,
  }), [inventory.devices, selectedDeviceId, sessionStatus]);

  const setStatus = useCallback((update: SetStateAction<DeviceStatus>) => {
    setSessionStatus((current) => {
      const next = typeof update === "function" ? update(current) : update;
      return { ...next, devices: [] };
    });
  }, []);

  const connect = useCallback(async (deviceId: string) => {
    selectedDeviceIntentRef.current = deviceId;
    releaseControlsRef.current();
    const connected = await connectInventory(deviceId);
    selectedDeviceIntentRef.current = null;
    if (connected) {
      setSelectedDeviceId(deviceId);
      setSessionStatus((current) => ({
        ...current,
        status: "connecting to device...",
        phase: "connecting",
        active_device_id: deviceId,
        error: null,
      }));
    }
    return connected;
  }, [connectInventory]);

  const select = useCallback(async (deviceId: string) => {
    const device = inventory.devices.find((candidate) => candidate.id === deviceId);
    if (!device || device.pairing === "unpaired") return false;
    if (!isActiveSession(device)) return connect(deviceId);
    releaseControlsRef.current();
    selectedDeviceIntentRef.current = null;
    setSelectedDeviceId(deviceId);
    setSessionStatus((current) => ({
      ...current,
      status: device.session_status ?? "connecting to device...",
      phase: device.session_phase ?? "connecting",
      active_device_id: deviceId,
      active_udid: device.udid,
      error: device.session_error,
    }));
    return true;
  }, [connect, inventory.devices]);

  const reconnect = useCallback(async (deviceId = selectedDeviceId) => {
    if (!deviceId) return false;
    if (deviceId === selectedDeviceId) releaseControlsRef.current();
    return reconnectInventory(deviceId);
  }, [reconnectInventory, selectedDeviceId]);

  const disconnect = useCallback(async (deviceId: string) => {
    const isSelected = deviceId === selectedDeviceId;
    if (isSelected) releaseControlsRef.current();
    const disconnected = await disconnectInventory(deviceId);
    if (disconnected && isSelected) {
      selectedDeviceIntentRef.current = null;
      setSelectedDeviceId(null);
      setSessionStatus((current) => ({
        ...current,
        status: "stopping...",
        phase: "disconnecting",
        active_udid: null,
        active_device_id: null,
      }));
    }
    return disconnected;
  }, [disconnectInventory, selectedDeviceId]);

  const pair = useCallback(async (deviceId: string) => {
    const paired = await pairInventory(deviceId);
    if (paired) setSelectedDeviceId(deviceId);
    return paired;
  }, [pairInventory]);

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
