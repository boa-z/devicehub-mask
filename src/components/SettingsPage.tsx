import BugOutlined from "@ant-design/icons/es/icons/BugOutlined";
import FolderOpenOutlined from "@ant-design/icons/es/icons/FolderOpenOutlined";
import GithubOutlined from "@ant-design/icons/es/icons/GithubOutlined";
import ReloadOutlined from "@ant-design/icons/es/icons/ReloadOutlined";
import { openUrl } from "@tauri-apps/plugin-opener";
import { Button, Checkbox, Select, Slider, Space, Switch, Typography, message } from "antd";
import { useEffect, useState } from "react";
import { useTranslation } from "react-i18next";
import type { UpdateChannel } from "../buildInfo";
import { changeLanguage, normalizeLanguage, type SupportedLanguage } from "../i18n";
import { readHostCapabilities, runningInDesktopHost, type HostCapabilities } from "../hostApi";
import { showErrorMessage } from "../errorMessage";
import type { DeviceAudioPreferences } from "../deviceAudio";
import { defaultDeviceViewPreferences, type DeviceViewPreferences, type DeviceViewScale } from "../deviceViewPreferences";
import { performanceHudItems, type PerformanceHudItem, type PerformanceHudPreferences } from "../performanceHudPreferences";
import { openLogDirectory, readDiagnosticsStatus, setDebugLogging, type DiagnosticsStatus } from "../diagnostics";
import { useUpdates } from "../updateContext";
import {
  readAppSettings,
  setAudioEnabled,
  setClipboardSyncEnabled,
  type AppSettingsStatus,
} from "../appSettings";
import { UpdateButton } from "./UpdateButton";

type Props = {
  alwaysOnTop: boolean;
  systemFullscreen: boolean;
  deviceView: DeviceViewPreferences;
  performanceHud: PerformanceHudPreferences;
  audioPlayback: DeviceAudioPreferences;
  onAlwaysOnTopChange: () => void;
  onSystemFullscreenChange: () => void;
  onDeviceViewChange: (preferences: DeviceViewPreferences) => void;
  onPerformanceHudChange: (preferences: PerformanceHudPreferences) => void;
  onAudioPlaybackChange: (preferences: DeviceAudioPreferences) => void;
  onAudioEnabledChange: (enabled: boolean) => void;
};

export function SettingsPage({
  alwaysOnTop,
  systemFullscreen,
  deviceView,
  performanceHud,
  audioPlayback,
  onAlwaysOnTopChange,
  onSystemFullscreenChange,
  onDeviceViewChange,
  onPerformanceHudChange,
  onAudioPlaybackChange,
  onAudioEnabledChange,
}: Props) {
  const { t, i18n } = useTranslation();
  const language = normalizeLanguage(i18n.resolvedLanguage ?? i18n.language);
  const { automatic, buildInfo, channel, setAutomatic, setChannel } = useUpdates();
  const [diagnostics, setDiagnostics] = useState<DiagnosticsStatus | null>(null);
  const [diagnosticsBusy, setDiagnosticsBusy] = useState(false);
  const [appSettings, setAppSettings] = useState<AppSettingsStatus | null>(null);
  const [appSettingsBusy, setAppSettingsBusy] = useState(false);
  const [audioVolumeDraft, setAudioVolumeDraft] = useState<number | null>(null);
  const [hostCapabilities, setHostCapabilities] = useState<HostCapabilities | null>(null);
  useEffect(() => {
    void readHostCapabilities().then(setHostCapabilities).catch(() => undefined);
  }, []);
  useEffect(() => {
    void readDiagnosticsStatus()
      .then(setDiagnostics)
      .catch((error) => showErrorMessage(t("settings.diagnosticsUnavailable", { error: String(error) })));
  }, [t]);
  useEffect(() => {
    void readAppSettings()
      .then((settings) => {
        setAppSettings(settings);
        onAudioEnabledChange(settings.audio_enabled);
      })
      .catch((error) => showErrorMessage(t("settings.appSettingsUnavailable", { error: String(error) })));
  }, [onAudioEnabledChange, t]);

  const changeAudioEnabled = async (enabled: boolean) => {
    setAppSettingsBusy(true);
    try {
      const settings = await setAudioEnabled(enabled);
      setAppSettings(settings);
      onAudioEnabledChange(settings.audio_enabled);
      message.success(t("settings.deviceAudioChanged"));
    } catch (error) {
      showErrorMessage(t("settings.appSettingsUnavailable", { error: String(error) }));
    } finally {
      setAppSettingsBusy(false);
    }
  };

  const changeClipboardSyncEnabled = async (enabled: boolean) => {
    setAppSettingsBusy(true);
    try {
      setAppSettings(await setClipboardSyncEnabled(enabled));
      message.success(t("settings.clipboardSyncChanged"));
    } catch (error) {
      showErrorMessage(t("settings.appSettingsUnavailable", { error: String(error) }));
    } finally {
      setAppSettingsBusy(false);
    }
  };

  const changeDebugLogging = async (enabled: boolean) => {
    setDiagnosticsBusy(true);
    try {
      setDiagnostics(await setDebugLogging(enabled));
    } catch (error) {
      showErrorMessage(t("settings.diagnosticsUnavailable", { error: String(error) }));
    } finally {
      setDiagnosticsBusy(false);
    }
  };

  const showLogDirectory = async () => {
    try {
      await openLogDirectory();
    } catch (error) {
      showErrorMessage(t("settings.diagnosticsUnavailable", { error: String(error) }));
    }
  };

  const openRepository = async () => {
    try {
      if (runningInDesktopHost()) {
        await openUrl("https://github.com/boa-z/devicehub-mask");
      } else {
        window.open("https://github.com/boa-z/devicehub-mask", "_blank", "noopener,noreferrer");
      }
    } catch (error) {
      showErrorMessage(t("settings.openRepositoryFailed", { error: String(error) }));
    }
  };

  return (
    <section className="settings-page">
      <header>
        <Typography.Title level={3}>{t("settings.title")}</Typography.Title>
      </header>
      <div className="settings-section">
        <Typography.Title level={5}>{t("settings.appearance")}</Typography.Title>
        <label>
          <span>{t("settings.language")}</span>
          <Select<SupportedLanguage>
            className="language-select"
            value={language}
            options={[
              { value: "zh-CN", label: t("settings.languages.zhCN") },
              { value: "en-US", label: t("settings.languages.enUS") },
            ]}
            onChange={(value) => void changeLanguage(value)}
          />
        </label>
      </div>
      <div className="settings-section">
        <Typography.Title level={5}>{t("settings.window")}</Typography.Title>
        <label><span>{t("settings.alwaysOnTop")}</span><Switch checked={alwaysOnTop} disabled={hostCapabilities?.always_on_top === false} onChange={onAlwaysOnTopChange} /></label>
        <label><span>{t("settings.systemFullscreen")}</span><Switch checked={systemFullscreen} disabled={hostCapabilities?.system_fullscreen === false} onChange={onSystemFullscreenChange} /></label>
        <label><span>{t("settings.deviceInspector")}</span><Switch checked={deviceView.deviceInspectorVisible} onChange={(deviceInspectorVisible) => onDeviceViewChange({ ...deviceView, deviceInspectorVisible })} /></label>
        <label><span>{t("settings.mappingInspector")}</span><Switch checked={deviceView.mappingInspectorVisible} onChange={(mappingInspectorVisible) => onDeviceViewChange({ ...deviceView, mappingInspectorVisible })} /></label>
      </div>
      <div className="settings-section">
        <Typography.Title level={5}>{t("settings.deviceControl")}</Typography.Title>
        <label>
          <span>{t("settings.defaultDisplayScale")}</span>
          <Select<DeviceViewScale>
            className="device-view-scale-select"
            value={deviceView.scale}
            options={[
              { value: "fit", label: t("device.fitWindow") },
              { value: "0.25", label: "25%" },
              { value: "0.5", label: "50%" },
              { value: "0.75", label: "75%" },
              { value: "1", label: t("device.actualSize") },
              { value: "1.25", label: "125%" },
              { value: "1.5", label: "150%" },
              { value: "2", label: "200%" },
            ]}
            onChange={(scale) => onDeviceViewChange({ ...deviceView, scale })}
          />
        </label>
        <label><span>{t("settings.showControlOverlay")}</span><Switch checked={deviceView.controlOverlayVisible} onChange={(controlOverlayVisible) => onDeviceViewChange({ ...deviceView, controlOverlayVisible })} /></label>
        <label><span>{t("settings.lockRotationControls")}</span><Switch checked={deviceView.rotationControlsLocked} onChange={(rotationControlsLocked) => onDeviceViewChange({ ...deviceView, rotationControlsLocked })} /></label>
        <label><span>{t("settings.fullscreenToolbarAutoHide")}</span><Switch checked={deviceView.fullscreenToolbarAutoHide} onChange={(fullscreenToolbarAutoHide) => onDeviceViewChange({ ...deviceView, fullscreenToolbarAutoHide })} /></label>
        <label>
          <span>{t("settings.toolbarLayout")}</span>
          <Button
            icon={<ReloadOutlined />}
            onClick={() => onDeviceViewChange({
              ...deviceView,
              fullscreenHardwareToolbarDock: defaultDeviceViewPreferences.fullscreenHardwareToolbarDock,
              fullscreenFunctionToolbarDock: defaultDeviceViewPreferences.fullscreenFunctionToolbarDock,
              fullscreenToolbarsAttached: defaultDeviceViewPreferences.fullscreenToolbarsAttached,
            })}
          >
            {t("settings.resetToolbarLayout")}
          </Button>
        </label>
      </div>
      <div className="settings-section">
        <Typography.Title level={5}>{t("settings.audio")}</Typography.Title>
        <label>
          <span>{t("settings.deviceAudioEnabled")}</span>
          <Switch
            checked={appSettings?.audio_enabled ?? false}
            disabled={!appSettings || hostCapabilities?.device_audio === false}
            loading={appSettingsBusy}
            onChange={(enabled) => void changeAudioEnabled(enabled)}
          />
        </label>
        <label><span>{t("settings.deviceAudioMuted")}</span><Switch checked={audioPlayback.muted} disabled={hostCapabilities?.device_audio === false} onChange={(muted) => onAudioPlaybackChange({ ...audioPlayback, muted })} /></label>
        <label>
          <span>{t("settings.deviceAudioVolume")}</span>
          <Slider
            min={0}
            max={100}
            value={audioVolumeDraft ?? Math.round(audioPlayback.volume * 100)}
            disabled={audioPlayback.muted || hostCapabilities?.device_audio === false}
            onChange={setAudioVolumeDraft}
            onChangeComplete={(volume) => {
              setAudioVolumeDraft(null);
              onAudioPlaybackChange({ ...audioPlayback, volume: volume / 100 });
            }}
          />
        </label>
        <Typography.Text type="secondary">{t("settings.deviceAudioHint")}</Typography.Text>
      </div>
      <div className="settings-section">
        <Typography.Title level={5}>{t("settings.clipboard")}</Typography.Title>
        <label>
          <span>{t("settings.clipboardSyncEnabled")}</span>
          <Switch
            checked={appSettings?.clipboard_sync_enabled ?? false}
            disabled={!appSettings || hostCapabilities?.clipboard_sync === false}
            loading={appSettingsBusy}
            onChange={(enabled) => void changeClipboardSyncEnabled(enabled)}
          />
        </label>
        <Typography.Text type="secondary">{t("settings.clipboardSyncHint")}</Typography.Text>
      </div>
      <div className="settings-section performance-hud-settings">
        <Typography.Title level={5}>{t("settings.performanceHud")}</Typography.Title>
        <label>
          <span>{t("settings.performanceHudEnabled")}</span>
          <Switch
            checked={performanceHud.enabled}
            onChange={(enabled) => onPerformanceHudChange({ ...performanceHud, enabled })}
          />
        </label>
        <Typography.Text type="secondary">{t("settings.performanceHudHint")}</Typography.Text>
        <Typography.Text className="performance-hud-items-label">{t("settings.performanceHudItems")}</Typography.Text>
        <Checkbox.Group
          className="performance-hud-items"
          value={performanceHud.items}
          options={performanceHudItems.map((value) => ({ value, label: t(`performance.hud.items.${value}`) }))}
          onChange={(values) => onPerformanceHudChange({ ...performanceHud, items: values as PerformanceHudItem[] })}
        />
      </div>
      <div className="settings-section">
        <Typography.Title level={5}>{t("settings.updates")}</Typography.Title>
        <label>
          <span>{t("update.channel")}</span>
          <Select<UpdateChannel>
            disabled={hostCapabilities?.app_updates === false}
            value={channel}
            options={[
              { value: "stable", label: t("update.channels.stable") },
              { value: "nightly", label: t("update.channels.nightly") },
            ]}
            onChange={setChannel}
          />
        </label>
        <label>
          <span>{t("update.automatic")}</span>
          <Switch checked={automatic} disabled={hostCapabilities?.app_updates === false} onChange={setAutomatic} />
        </label>
        <label>
          <span>{t("update.manual")}</span>
          <UpdateButton />
        </label>
      </div>
      <div className="settings-section diagnostics-settings">
        <Typography.Title level={5}>{t("settings.diagnostics")}</Typography.Title>
        <label>
          <span>{t("settings.debugLogging")}</span>
          <Switch
            checked={diagnostics?.debug_enabled ?? false}
            disabled={!diagnostics || diagnostics.custom_filter}
            loading={diagnosticsBusy}
            onChange={(enabled) => void changeDebugLogging(enabled)}
          />
        </label>
        {diagnostics?.custom_filter && (
          <Typography.Text type="warning">{t("settings.customLogFilter")}</Typography.Text>
        )}
        <label>
          <span>{t("settings.logFiles")}</span>
          <Button icon={<FolderOpenOutlined />} disabled={!diagnostics?.file_logging} onClick={() => void showLogDirectory()}>
            {t("settings.openLogDirectory")}
          </Button>
        </label>
        <div className="diagnostics-detail">
          <Typography.Text type="secondary">{t("settings.logFilter")}</Typography.Text>
          <Typography.Text code copyable>{diagnostics?.filter ?? "-"}</Typography.Text>
          <Typography.Text type="secondary">{t("settings.runId")}</Typography.Text>
          <Typography.Text code copyable>{diagnostics?.run_id ?? "-"}</Typography.Text>
          <Typography.Text type="secondary">{t("settings.droppedLogs")}</Typography.Text>
          <Typography.Text>{diagnostics?.dropped_log_lines ?? 0}</Typography.Text>
        </div>
        <Space><BugOutlined /><Typography.Text type="secondary">{t("settings.debugLoggingHint")}</Typography.Text></Space>
      </div>
      <div className="settings-section">
        <Typography.Title level={5}>{t("settings.about")}</Typography.Title>
        <label><span>{t("settings.version")}</span><Typography.Text code>{buildInfo?.version ?? "-"}</Typography.Text></label>
        <label><span>{t("settings.build")}</span><Typography.Text code>{buildInfo?.build ?? "-"}</Typography.Text></label>
        <label><span>{t("settings.commit")}</span><Typography.Text code copyable={Boolean(buildInfo?.commit)}>{buildInfo?.commit ?? "-"}</Typography.Text></label>
        <label><span>{t("settings.repository")}</span><Button icon={<GithubOutlined />} onClick={() => void openRepository()}>{t("settings.openGithub")}</Button></label>
      </div>
    </section>
  );
}
