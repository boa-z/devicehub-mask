import AppstoreOutlined from "@ant-design/icons/es/icons/AppstoreOutlined";
import CloseOutlined from "@ant-design/icons/es/icons/CloseOutlined";
import DownloadOutlined from "@ant-design/icons/es/icons/DownloadOutlined";
import MobileOutlined from "@ant-design/icons/es/icons/MobileOutlined";
import ReloadOutlined from "@ant-design/icons/es/icons/ReloadOutlined";
import SearchOutlined from "@ant-design/icons/es/icons/SearchOutlined";
import SwapOutlined from "@ant-design/icons/es/icons/SwapOutlined";
import { Input, Select } from "antd";
import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import { useTranslation } from "react-i18next";
import "./KeymapCatalogModal.css";
import { filterKeymapCatalogEntries, keymapCatalogMatchLevel } from "../keymapCatalog";
import { uniqueImportedProfileName } from "../mappingImport";
import { readBackendJson } from "../shared/backend/response";
import type {
  DeviceApp,
  DeviceDetails,
  KeymapCatalog,
  KeymapCatalogDeviceContext,
  KeymapCatalogEntry,
  KeymapCatalogInstall,
  KeymapCatalogSource,
  Orientation,
  ProfileResolution,
} from "../types";

type Request = (path: string, init?: RequestInit) => Promise<Response>;

type CatalogSelectOption = {
  value: string;
  label: string;
  searchText: string;
  detail?: string;
};

type DeviceTargetOption = CatalogSelectOption & {
  context: KeymapCatalogDeviceContext | null;
};

type Props = {
  open: boolean;
  request: Request;
  profiles: string[];
  activeDeviceId: string | null;
  frameSize: ProfileResolution;
  orientation: Orientation;
  hasFrame: boolean;
  onClose: () => void;
  onInstalled: (name: string) => Promise<void>;
};

function entryResolution(entry: KeymapCatalogEntry) {
  return `${entry.match.stream_resolution.width} x ${entry.match.stream_resolution.height}`;
}

function sourceHost(source: KeymapCatalogSource) {
  try {
    return new URL(source.url).host;
  } catch {
    return source.url;
  }
}

function deviceTargetKey(device: KeymapCatalogDeviceContext) {
  return [
    device.product_type ?? "any",
    device.frame_size.width,
    device.frame_size.height,
    device.orientation,
  ].join("|");
}

export function KeymapCatalogModal({
  open,
  request,
  profiles,
  activeDeviceId,
  frameSize,
  orientation,
  hasFrame,
  onClose,
  onInstalled,
}: Props) {
  const { t } = useTranslation();
  const [catalog, setCatalog] = useState<KeymapCatalog | null>(null);
  const [source, setSource] = useState<KeymapCatalogSource | null>(null);
  const [sourceDraft, setSourceDraft] = useState("");
  const [sourceOpen, setSourceOpen] = useState(false);
  const [sourceSaving, setSourceSaving] = useState(false);
  const [sourceError, setSourceError] = useState<string | null>(null);
  const [apps, setApps] = useState<DeviceApp[]>([]);
  const [details, setDetails] = useState<DeviceDetails | null>(null);
  const [selectedBundleId, setSelectedBundleId] = useState<string | null>(null);
  const [selectedDeviceTarget, setSelectedDeviceTarget] = useState("current");
  const [catalogQuery, setCatalogQuery] = useState("");
  const [loading, setLoading] = useState(false);
  const [installing, setInstalling] = useState<string | null>(null);
  const [error, setError] = useState<string | null>(null);
  const closeButtonRef = useRef<HTMLButtonElement>(null);
  const refreshGenerationRef = useRef(0);

  const refreshCatalog = useCallback(async () => {
    const generation = refreshGenerationRef.current + 1;
    refreshGenerationRef.current = generation;
    setLoading(true);
    setError(null);
    try {
      const nextCatalog = await readBackendJson<KeymapCatalog>(await request("/api/keymap-catalog/refresh", {
        method: "POST",
      }));
      if (generation === refreshGenerationRef.current) setCatalog(nextCatalog);
    } catch (nextError) {
      if (generation === refreshGenerationRef.current) setError(String(nextError));
      try {
        const cachedCatalog = await readBackendJson<KeymapCatalog>(await request("/api/keymap-catalog"));
        if (generation === refreshGenerationRef.current) setCatalog(cachedCatalog);
      } catch {
        if (generation === refreshGenerationRef.current) setCatalog(null);
      }
    } finally {
      if (generation === refreshGenerationRef.current) setLoading(false);
    }
  }, [request]);

  const loadSource = useCallback(async () => {
    try {
      const next = await readBackendJson<KeymapCatalogSource>(await request("/api/keymap-catalog/source"));
      setSource(next);
      setSourceDraft(next.url);
      setSourceError(null);
    } catch (nextError) {
      setSourceError(t("profile.catalogSourceSaveFailed", { error: String(nextError) }));
    }
  }, [request, t]);

  const loadDeviceContext = useCallback(async () => {
    if (!activeDeviceId) {
      setApps([]);
      setDetails(null);
      return;
    }
    const [appsResult, detailsResult] = await Promise.allSettled([
      request("/api/device/apps").then((response) => readBackendJson<DeviceApp[]>(response)),
      request("/api/device/details").then((response) => readBackendJson<DeviceDetails>(response)),
    ]);
    setApps(appsResult.status === "fulfilled" ? appsResult.value : []);
    setDetails(detailsResult.status === "fulfilled" ? detailsResult.value : null);
  }, [activeDeviceId, request]);

  useEffect(() => {
    if (!open) return;
    void refreshCatalog();
    void loadDeviceContext();
    void loadSource();
  }, [loadDeviceContext, loadSource, open, refreshCatalog]);

  useEffect(() => {
    if (!open) return;
    closeButtonRef.current?.focus();
  }, [open]);

  useEffect(() => {
    if (!open) return;
    const onKeyDown = (event: KeyboardEvent) => {
      if (event.key === "Escape") onClose();
    };
    window.addEventListener("keydown", onKeyDown);
    return () => window.removeEventListener("keydown", onKeyDown);
  }, [onClose, open]);

  useEffect(() => {
    if (selectedBundleId || !catalog || apps.length === 0) return;
    const matchingApp = apps.find((app) => catalog.entries.some((entry) => entry.match.bundle_ids.includes(app.bundle_id)));
    if (matchingApp) setSelectedBundleId(matchingApp.bundle_id);
  }, [apps, catalog, selectedBundleId]);

  const device = useMemo<KeymapCatalogDeviceContext | null>(() => {
    if (!hasFrame) return null;
    return {
      product_type: details?.product_type ?? null,
      frame_size: { width: frameSize.width, height: frameSize.height },
      orientation,
    };
  }, [details?.product_type, frameSize.height, frameSize.width, hasFrame, orientation]);
  const appOptions = useMemo<CatalogSelectOption[]>(() => [
    {
      value: "",
      label: t("profile.catalogAllApps"),
      searchText: t("profile.catalogAllApps"),
    },
    ...apps.map((app) => ({
      value: app.bundle_id,
      label: app.name,
      detail: app.bundle_id,
      searchText: `${app.name} ${app.bundle_id}`,
    })),
  ], [apps, t]);
  const deviceOptions = useMemo<DeviceTargetOption[]>(() => {
    const options: DeviceTargetOption[] = [{
      value: "all",
      label: t("profile.catalogAllDevices"),
      searchText: t("profile.catalogAllDevices"),
      context: null,
    }];
    const currentKey = device ? deviceTargetKey(device) : null;
    if (device) {
      const detail = `${device.product_type ?? t("profile.catalogAnyDevice")} | ${device.frame_size.width} x ${device.frame_size.height} | ${t(`profile.catalogOrientations.${device.orientation}`)}`;
      options.push({
        value: "current",
        label: t("profile.catalogCurrentDevice"),
        detail,
        searchText: `${t("profile.catalogCurrentDevice")} ${detail}`,
        context: device,
      });
    }

    const targets = new Map<string, KeymapCatalogDeviceContext>();
    for (const entry of catalog?.entries ?? []) {
      const productTypes = entry.match.product_types.length > 0 ? entry.match.product_types : [null];
      for (const productType of productTypes) {
        const context = {
          product_type: productType,
          frame_size: entry.match.stream_resolution,
          orientation: entry.match.orientation,
        };
        targets.set(deviceTargetKey(context), context);
      }
    }
    const catalogTargets = Array.from(targets.entries())
      .filter(([key]) => key !== currentKey)
      .map(([key, context]) => {
        const label = context.product_type ?? t("profile.catalogAnyDevice");
        const detail = `${context.frame_size.width} x ${context.frame_size.height} | ${t(`profile.catalogOrientations.${context.orientation}`)}`;
        return {
          value: `target:${key}`,
          label,
          detail,
          searchText: `${label} ${detail}`,
          context,
        };
      })
      .sort((left, right) => left.searchText.localeCompare(right.searchText));
    return [...options, ...catalogTargets];
  }, [catalog?.entries, device, t]);
  const activeDeviceTarget = selectedDeviceTarget === "current" && !device ? "all" : selectedDeviceTarget;
  const selectedDevice = activeDeviceTarget === "current"
    ? device
    : deviceOptions.find((option) => option.value === activeDeviceTarget)?.context ?? null;
  const matchDevice = selectedDevice ?? device;

  useEffect(() => {
    if (selectedDeviceTarget === "current" || selectedDeviceTarget === "all") return;
    if (deviceOptions.some((option) => option.value === selectedDeviceTarget)) return;
    setSelectedDeviceTarget(device ? "current" : "all");
  }, [device, deviceOptions, selectedDeviceTarget]);

  const entries = useMemo(() => filterKeymapCatalogEntries(
    catalog?.entries ?? [],
    selectedBundleId,
    selectedDevice,
    catalogQuery,
  ), [catalog?.entries, catalogQuery, selectedBundleId, selectedDevice]);

  const install = async (entry: KeymapCatalogEntry) => {
    const name = uniqueImportedProfileName(`${entry.slug}.json`, profiles);
    setInstalling(entry.id);
    try {
      const installed = await readBackendJson<KeymapCatalogInstall>(await request(
        `/api/keymap-catalog/entries/${encodeURIComponent(entry.id)}/install`,
        {
          method: "POST",
          headers: { "content-type": "application/json" },
          body: JSON.stringify({ name }),
        },
      ));
      await onInstalled(installed.name);
      onClose();
    } catch (installError) {
      setError(t("profile.catalogInstallFailed", { error: String(installError) }));
    } finally {
      setInstalling(null);
    }
  };

  const saveSource = async (url: string | null) => {
    setSourceSaving(true);
    setSourceError(null);
    try {
      const next = await readBackendJson<KeymapCatalogSource>(await request("/api/keymap-catalog/source", {
        method: "PUT",
        headers: { "content-type": "application/json" },
        body: JSON.stringify({ url }),
      }));
      setSource(next);
      setSourceDraft(next.url);
      setCatalog(null);
      await refreshCatalog();
    } catch (saveError) {
      setSourceError(t("profile.catalogSourceSaveFailed", { error: String(saveError) }));
    } finally {
      setSourceSaving(false);
    }
  };

  const resetFilters = () => {
    setSelectedBundleId(null);
    setSelectedDeviceTarget("all");
    setCatalogQuery("");
  };

  if (!open) return null;

  const noPublishedEntries = catalog?.entries.length === 0;
  const emptyMessage = !catalog
    ? t("profile.catalogLoading")
    : noPublishedEntries
      ? t("profile.catalogNoPublished")
      : t("profile.catalogEmpty");

  return (
    <div className="keymap-catalog-backdrop" role="presentation" onMouseDown={(event) => {
      if (event.target === event.currentTarget) onClose();
    }}>
      <section className="keymap-catalog-modal" role="dialog" aria-modal="true" aria-labelledby="keymap-catalog-title">
        <header className="keymap-catalog-header">
          <div className="keymap-catalog-heading">
            <h2 id="keymap-catalog-title">{t("profile.catalogTitle")}</h2>
            {source && <span className="keymap-catalog-source-host" title={source.url}>{sourceHost(source)}</span>}
          </div>
          <div className="keymap-catalog-header-actions">
            <button
              className="keymap-catalog-source-toggle"
              type="button"
              aria-expanded={sourceOpen}
              onClick={() => setSourceOpen((value) => !value)}
            ><SwapOutlined /><span>{t("profile.catalogRepository")}</span></button>
            <button ref={closeButtonRef} className="keymap-catalog-close" type="button" aria-label={t("common.cancel")} title={t("common.cancel")} onClick={onClose}><CloseOutlined /></button>
          </div>
        </header>
        {sourceOpen && <section className="keymap-catalog-source-settings" aria-label={t("profile.catalogRepository")}>
          <div className="keymap-catalog-source-heading">
            <div>
              <span>{t("profile.catalogRepository")}</span>
              <strong>{source ? (source.is_default ? t("profile.catalogOfficialSource") : t("profile.catalogCustomSource")) : t("profile.catalogLoading")}</strong>
            </div>
            {source && <span className="keymap-catalog-source-status">{source.is_default ? t("profile.catalogOfficialActive") : t("profile.catalogCustomActive")}</span>}
          </div>
          <div className="keymap-catalog-official-source">
            <div>
              <strong>{t("profile.catalogOfficialSource")}</strong>
              {source && <span title={source.default_url}>{source.default_url}</span>}
            </div>
            <button
              className="keymap-catalog-secondary-action"
              type="button"
              disabled={sourceSaving || !source || source.is_default}
              onClick={() => void saveSource(null)}
            ><SwapOutlined /><span>{source?.is_default ? t("profile.catalogOfficialActive") : t("profile.catalogUseDefaultSource")}</span></button>
          </div>
          <form className="keymap-catalog-source-form" onSubmit={(event) => {
            event.preventDefault();
            void saveSource(sourceDraft.trim() || null);
          }}>
            <label className="keymap-catalog-source-field">
              <span>{t("profile.catalogCustomAddress")}</span>
              <input
                type="url"
                value={sourceDraft}
                placeholder={t("profile.catalogAddressHint")}
                autoComplete="off"
                spellCheck={false}
                onChange={(event) => setSourceDraft(event.target.value)}
              />
            </label>
            <div className="keymap-catalog-source-actions">
              <button className="keymap-catalog-action" type="submit" disabled={sourceSaving}><SwapOutlined /><span>{t("profile.catalogSaveSource")}</span></button>
            </div>
          </form>
          {sourceError && <div className="keymap-catalog-source-error" role="alert">{sourceError}</div>}
        </section>}
        <div className="keymap-catalog">
          <div className="keymap-catalog-toolbar">
            <div className="keymap-catalog-control keymap-catalog-search">
              <span id="keymap-catalog-search-label" className="keymap-catalog-control-label"><SearchOutlined />{t("profile.catalogSearch")}</span>
              <Input
                allowClear
                aria-labelledby="keymap-catalog-search-label"
                value={catalogQuery}
                placeholder={t("profile.catalogSearchHint")}
                onChange={(event) => setCatalogQuery(event.target.value)}
              />
            </div>
            <div className="keymap-catalog-control">
              <span id="keymap-catalog-app-label" className="keymap-catalog-control-label"><AppstoreOutlined />{t("profile.catalogApp")}</span>
              <Select<string>
                className="keymap-catalog-select"
                aria-labelledby="keymap-catalog-app-label"
                value={selectedBundleId ?? ""}
                showSearch
                optionFilterProp="searchText"
                popupClassName="keymap-catalog-select-dropdown"
                options={appOptions}
                onChange={(value) => setSelectedBundleId(value || null)}
                optionRender={(option) => {
                  const data = option.data as CatalogSelectOption;
                  return <div className="keymap-catalog-option"><strong>{data.label}</strong>{data.detail && <span>{data.detail}</span>}</div>;
                }}
              />
            </div>
            <div className="keymap-catalog-control">
              <span id="keymap-catalog-device-label" className="keymap-catalog-control-label"><MobileOutlined />{t("profile.catalogDevice")}</span>
              <Select<string>
                className="keymap-catalog-select"
                aria-labelledby="keymap-catalog-device-label"
                value={activeDeviceTarget}
                showSearch
                optionFilterProp="searchText"
                popupClassName="keymap-catalog-select-dropdown"
                options={deviceOptions}
                onChange={setSelectedDeviceTarget}
                optionRender={(option) => {
                  const data = option.data as DeviceTargetOption;
                  return <div className="keymap-catalog-option"><strong>{data.label}</strong>{data.detail && <span>{data.detail}</span>}</div>;
                }}
              />
            </div>
            <button className="keymap-catalog-refresh" type="button" aria-label={t("profile.catalogRefresh")} title={t("profile.catalogRefresh")} disabled={loading || sourceSaving} onClick={() => void refreshCatalog()}><ReloadOutlined spin={loading} /></button>
          </div>
          <div className="keymap-catalog-summary">
            <div className="keymap-catalog-context">
              {catalog && <span className="keymap-catalog-repository">{catalog.repository.name}</span>}
              {matchDevice && <span className="keymap-catalog-tag">{t("profile.catalogFrame", { width: matchDevice.frame_size.width, height: matchDevice.frame_size.height })}</span>}
              {matchDevice && <span className="keymap-catalog-tag">{t(`profile.catalogOrientations.${matchDevice.orientation}`)}</span>}
              {matchDevice?.product_type && <span className="keymap-catalog-tag">{matchDevice.product_type}</span>}
            </div>
            {catalog && <span className="keymap-catalog-result-count">{t("profile.catalogResultCount", { count: entries.length })}</span>}
          </div>
          {error && <div className="keymap-catalog-alert" role="alert">
            <strong>{t("profile.catalogRefreshFailed")}</strong>
            <span>{error}</span>
          </div>}
          {loading && catalog === null ? (
            <div className="keymap-catalog-empty" aria-live="polite"><strong>{t("profile.catalogLoading")}</strong></div>
          ) : entries.length === 0 ? (
            <div className="keymap-catalog-empty" aria-live="polite">
              <strong>{emptyMessage}</strong>
              {catalog && !noPublishedEntries && <button className="keymap-catalog-empty-action" type="button" onClick={resetFilters}>{t("profile.catalogClearFilters")}</button>}
              {noPublishedEntries && <button className="keymap-catalog-empty-action" type="button" onClick={() => setSourceOpen(true)}>{t("profile.catalogConfigureSource")}</button>}
            </div>
          ) : (
            <ul className="keymap-catalog-list">
              {entries.map((entry) => {
                const level = keymapCatalogMatchLevel(entry, selectedBundleId, matchDevice);
                return (
                  <li key={entry.id} className="keymap-catalog-entry">
                    <div className="keymap-catalog-entry-copy">
                      <strong title={entry.title}>{entry.title}</strong>
                      {entry.description && <span>{entry.description}</span>}
                    </div>
                    <div className="keymap-catalog-entry-details">
                      <div className="keymap-catalog-tags">
                        <span className={`keymap-catalog-tag is-${level}`}>{t(`profile.catalogMatch.${level}`)}</span>
                        <span className="keymap-catalog-tag">{entryResolution(entry)}</span>
                        <span className="keymap-catalog-tag">{t(`profile.catalogOrientations.${entry.match.orientation}`)}</span>
                        {entry.match.product_types.map((productType) => <span key={productType} className="keymap-catalog-tag">{productType}</span>)}
                        {entry.author && <span className="keymap-catalog-tag">{entry.author}</span>}
                      </div>
                      <button
                        className="keymap-catalog-download"
                        type="button"
                        disabled={installing !== null}
                        onClick={() => void install(entry)}
                      ><DownloadOutlined /><span>{installing === entry.id ? t("profile.catalogDownloading") : t("profile.catalogInstall")}</span></button>
                    </div>
                  </li>
                );
              })}
            </ul>
          )}
        </div>
      </section>
    </div>
  );
}
