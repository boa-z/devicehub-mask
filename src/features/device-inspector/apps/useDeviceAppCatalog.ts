import { useCallback, useEffect, useRef, useState } from "react";
import { deviceAppScopeQuery } from "../../../deviceInspector";
import { readBackendJson } from "../../../shared/backend/response";
import type { DeviceApp } from "../../../types";

type Request = (path: string, init?: RequestInit) => Promise<Response>;
type AppScope = "system" | "clips";

const APP_CATALOG_CACHE_MS = 30_000;

export type DeviceAppCatalog = {
  apps: DeviceApp[];
  showSystemApps: boolean;
  showAppClips: boolean;
  scopesLoading: boolean;
  load: (includeSystem?: boolean, includeAppClips?: boolean, force?: boolean) => Promise<boolean>;
  toggleScope: (scope: AppScope) => Promise<void>;
};

export function useDeviceAppCatalog(
  deviceScope: string | null,
  enabled: boolean,
  request: Request,
): DeviceAppCatalog {
  const [apps, setApps] = useState<DeviceApp[]>([]);
  const [showSystemApps, setShowSystemApps] = useState(false);
  const [showAppClips, setShowAppClips] = useState(false);
  const [scopesLoading, setScopesLoading] = useState(false);
  const requestGeneration = useRef(0);
  const scopeGeneration = useRef(0);
  const scopeBusy = useRef(false);
  const abortController = useRef<AbortController | null>(null);
  const cache = useRef(new Map<string, { loadedAt: number; apps: DeviceApp[] }>());
  const showSystemAppsRef = useRef(false);
  const showAppClipsRef = useRef(false);

  const load = useCallback(async (
    includeSystem = showSystemAppsRef.current,
    includeAppClips = showAppClipsRef.current,
    force = false,
  ) => {
    if (!deviceScope) return false;
    if (force) cache.current.clear();
    const cacheKey = `${deviceScope}:${includeSystem}:${includeAppClips}`;
    const cached = cache.current.get(cacheKey);
    if (!force && cached && performance.now() - cached.loadedAt < APP_CATALOG_CACHE_MS) {
      setApps(cached.apps);
      return true;
    }

    abortController.current?.abort();
    const controller = new AbortController();
    abortController.current = controller;
    const generation = ++requestGeneration.current;
    try {
      const suffix = deviceAppScopeQuery(includeSystem, includeAppClips);
      const nextApps = await readBackendJson<DeviceApp[]>(await request(`/api/device/apps${suffix}`, {
        signal: controller.signal,
      }));
      if (requestGeneration.current !== generation || controller.signal.aborted) return false;
      cache.current.set(cacheKey, { loadedAt: performance.now(), apps: nextApps });
      setApps(nextApps);
      return true;
    } catch (error) {
      if (controller.signal.aborted) return false;
      throw error;
    } finally {
      if (abortController.current === controller) abortController.current = null;
    }
  }, [deviceScope, request]);

  const toggleScope = useCallback(async (scope: AppScope) => {
    if (scopeBusy.current) return;
    const nextSystem = scope === "system" ? !showSystemAppsRef.current : showSystemAppsRef.current;
    const nextAppClips = scope === "clips" ? !showAppClipsRef.current : showAppClipsRef.current;
    const generation = ++scopeGeneration.current;
    scopeBusy.current = true;
    setScopesLoading(true);
    try {
      if (await load(nextSystem, nextAppClips)) {
        showSystemAppsRef.current = nextSystem;
        showAppClipsRef.current = nextAppClips;
        setShowSystemApps(nextSystem);
        setShowAppClips(nextAppClips);
      }
    } finally {
      if (scopeGeneration.current === generation) {
        scopeBusy.current = false;
        setScopesLoading(false);
      }
    }
  }, [load]);

  useEffect(() => {
    requestGeneration.current += 1;
    scopeGeneration.current += 1;
    abortController.current?.abort();
    abortController.current = null;
    cache.current.clear();
    scopeBusy.current = false;
    showSystemAppsRef.current = false;
    showAppClipsRef.current = false;
    setApps([]);
    setShowSystemApps(false);
    setShowAppClips(false);
    setScopesLoading(false);
  }, [deviceScope, request]);

  useEffect(() => {
    if (enabled) return;
    requestGeneration.current += 1;
    abortController.current?.abort();
    abortController.current = null;
  }, [enabled]);

  useEffect(() => () => abortController.current?.abort(), []);

  return {
    apps,
    showSystemApps,
    showAppClips,
    scopesLoading,
    load,
    toggleScope,
  };
}
