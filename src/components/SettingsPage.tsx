import { BugOutlined, FolderOpenOutlined, GithubOutlined } from "@ant-design/icons";
import { getVersion } from "@tauri-apps/api/app";
import { openUrl } from "@tauri-apps/plugin-opener";
import { Button, Checkbox, Select, Slider, Space, Switch, Tag, Typography, message } from "antd";
import { useEffect, useState } from "react";
import { useTranslation } from "react-i18next";
import { normalizeLanguage, type SupportedLanguage } from "../i18n";
import type { DeviceAudioPreferences } from "../deviceAudio";
import { type DeviceViewPreferences, type DeviceViewScale } from "../deviceViewPreferences";
import { performanceHudItems, type PerformanceHudItem, type PerformanceHudPreferences } from "../performanceHudPreferences";
import { openLogDirectory, readDiagnosticsStatus, setDebugLogging, type DiagnosticsStatus } from "../diagnostics";
import { useUpdates } from "../updateContext";
import {
  readVideoSettings,
  setAudioEnabled,
  setClipboardSyncEnabled,
  setVideoDecoderBackend,
  setVideoPixelFormat,
  type VideoDecoderBackend,
  type VideoPixelFormat,
  type VideoSettingsStatus,
} from "../videoSettings";
import { UpdateButton } from "./UpdateButton";

type Props = {
  alwaysOnTop: boolean;
  systemFullscreen: boolean;
  inspectorVisible: boolean;
  deviceView: DeviceViewPreferences;
  performanceHud: PerformanceHudPreferences;
  audioPlayback: DeviceAudioPreferences;
  onAlwaysOnTopChange: () => void;
  onSystemFullscreenChange: () => void;
  onInspectorVisibleChange: (visible: boolean) => void;
  onDeviceViewChange: (preferences: DeviceViewPreferences) => void;
  onPerformanceHudChange: (preferences: PerformanceHudPreferences) => void;
  onAudioPlaybackChange: (preferences: DeviceAudioPreferences) => void;
  onAudioEnabledChange: (enabled: boolean) => void;
};

export function SettingsPage({
  alwaysOnTop,
  systemFullscreen,
  inspectorVisible,
  deviceView,
  performanceHud,
  audioPlayback,
  onAlwaysOnTopChange,
  onSystemFullscreenChange,
  onInspectorVisibleChange,
  onDeviceViewChange,
  onPerformanceHudChange,
  onAudioPlaybackChange,
  onAudioEnabledChange,
}: Props) {
  const { t, i18n } = useTranslation();
  const language = normalizeLanguage(i18n.resolvedLanguage ?? i18n.language);
  const { automatic, setAutomatic } = useUpdates();
  const [version, setVersion] = useState("-");
  const [diagnostics, setDiagnostics] = useState<DiagnosticsStatus | null>(null);
  const [diagnosticsBusy, setDiagnosticsBusy] = useState(false);
  const [videoSettings, setVideoSettings] = useState<VideoSettingsStatus | null>(null);
  const [videoSettingsBusy, setVideoSettingsBusy] = useState(false);
  const [audioVolumeDraft, setAudioVolumeDraft] = useState<number | null>(null);
  useEffect(() => { void getVersion().then(setVersion); }, []);
  useEffect(() => {
    void readDiagnosticsStatus()
      .then(setDiagnostics)
      .catch((error) => message.error(t("settings.diagnosticsUnavailable", { error: String(error) })));
  }, [t]);
  useEffect(() => {
    void readVideoSettings()
      .then((settings) => {
        setVideoSettings(settings);
        onAudioEnabledChange(settings.audio_enabled);
      })
      .catch((error) => message.error(t("settings.videoSettingsUnavailable", { error: String(error) })));
  }, [onAudioEnabledChange, t]);

  const changeVideoPixelFormat = async (videoPixelFormat: VideoPixelFormat) => {
    setVideoSettingsBusy(true);
    try {
      setVideoSettings(await setVideoPixelFormat(videoPixelFormat));
      message.success(t("settings.videoPixelFormatChanged"));
    } catch (error) {
      message.error(t("settings.videoSettingsUnavailable", { error: String(error) }));
    } finally {
      setVideoSettingsBusy(false);
    }
  };

  const changeVideoDecoderBackend = async (videoDecoderBackend: VideoDecoderBackend) => {
    setVideoSettingsBusy(true);
    try {
      setVideoSettings(await setVideoDecoderBackend(videoDecoderBackend));
      message.success(t("settings.videoDecoderChanged"));
    } catch (error) {
      message.error(t("settings.videoSettingsUnavailable", { error: String(error) }));
    } finally {
      setVideoSettingsBusy(false);
    }
  };

  const changeAudioEnabled = async (enabled: boolean) => {
    setVideoSettingsBusy(true);
    try {
      const settings = await setAudioEnabled(enabled);
      setVideoSettings(settings);
      onAudioEnabledChange(settings.audio_enabled);
      message.success(t("settings.deviceAudioChanged"));
    } catch (error) {
      message.error(t("settings.videoSettingsUnavailable", { error: String(error) }));
    } finally {
      setVideoSettingsBusy(false);
    }
  };

  const changeClipboardSyncEnabled = async (enabled: boolean) => {
    setVideoSettingsBusy(true);
    try {
      setVideoSettings(await setClipboardSyncEnabled(enabled));
      message.success(t("settings.clipboardSyncChanged"));
    } catch (error) {
      message.error(t("settings.videoSettingsUnavailable", { error: String(error) }));
    } finally {
      setVideoSettingsBusy(false);
    }
  };

  const changeDebugLogging = async (enabled: boolean) => {
    setDiagnosticsBusy(true);
    try {
      setDiagnostics(await setDebugLogging(enabled));
    } catch (error) {
      message.error(t("settings.diagnosticsUnavailable", { error: String(error) }));
    } finally {
      setDiagnosticsBusy(false);
    }
  };

  const showLogDirectory = async () => {
    try {
      await openLogDirectory();
    } catch (error) {
      message.error(t("settings.diagnosticsUnavailable", { error: String(error) }));
    }
  };

  const openRepository = async () => {
    try {
      await openUrl("https://github.com/boa-z/devicehub-mask");
    } catch (error) {
      message.error(t("settings.openRepositoryFailed", { error: String(error) }));
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
            onChange={(value) => void i18n.changeLanguage(value)}
          />
        </label>
      </div>
      <div className="settings-section">
        <Typography.Title level={5}>{t("settings.window")}</Typography.Title>
        <label><span>{t("settings.alwaysOnTop")}</span><Switch checked={alwaysOnTop} onChange={onAlwaysOnTopChange} /></label>
        <label><span>{t("settings.systemFullscreen")}</span><Switch checked={systemFullscreen} onChange={onSystemFullscreenChange} /></label>
        <label><span>{t("settings.inspector")}</span><Switch checked={inspectorVisible} onChange={onInspectorVisibleChange} /></label>
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
      </div>
      <div className="settings-section">
        <Typography.Title level={5}>{t("settings.video")}</Typography.Title>
        <label>
          <Space size={8} wrap>
            <span>{t("settings.videoDecoder")}</span>
            <Tag color="warning">{t("settings.experimental")}</Tag>
          </Space>
          <Select<VideoDecoderBackend>
            className="video-format-select"
            value={videoSettings?.video_decoder_backend}
            disabled={!videoSettings}
            loading={videoSettingsBusy}
            options={[
              { value: "native", label: t("settings.videoDecoders.native") },
              { value: "browser", label: t("settings.videoDecoders.browser") },
            ]}
            onChange={(value) => void changeVideoDecoderBackend(value)}
          />
        </label>
        <Typography.Text type={videoSettings?.browser_decoder_fallback ? "warning" : "secondary"}>
          {videoSettings?.browser_decoder_fallback
            ? t("settings.videoDecoderFallback", { error: videoSettings.browser_decoder_fallback })
            : t("settings.videoDecoderHint")}
        </Typography.Text>
        <label>
          <Space size={8} wrap>
            <span>{t("settings.videoPixelFormat")}</span>
            <Tag color="warning">{t("settings.experimental")}</Tag>
          </Space>
          <Select<VideoPixelFormat>
            className="video-format-select"
            value={videoSettings?.video_pixel_format}
            disabled={!videoSettings || videoSettings.environment_override}
            loading={videoSettingsBusy}
            options={[
              { value: "rgb24", label: t("settings.videoFormats.rgb24") },
              { value: "yuv420p", label: t("settings.videoFormats.yuv420p") },
            ]}
            onChange={(value) => void changeVideoPixelFormat(value)}
          />
        </label>
        <Typography.Text type="secondary">
          {videoSettings?.environment_override
            ? t("settings.videoPixelFormatEnvironmentOverride")
            : t("settings.videoPixelFormatHint")}
        </Typography.Text>
      </div>
      <div className="settings-section">
        <Typography.Title level={5}>{t("settings.audio")}</Typography.Title>
        <label>
          <span>{t("settings.deviceAudioEnabled")}</span>
          <Switch
            checked={videoSettings?.audio_enabled ?? false}
            disabled={!videoSettings}
            loading={videoSettingsBusy}
            onChange={(enabled) => void changeAudioEnabled(enabled)}
          />
        </label>
        <label><span>{t("settings.deviceAudioMuted")}</span><Switch checked={audioPlayback.muted} onChange={(muted) => onAudioPlaybackChange({ ...audioPlayback, muted })} /></label>
        <label>
          <span>{t("settings.deviceAudioVolume")}</span>
          <Slider
            min={0}
            max={100}
            value={audioVolumeDraft ?? Math.round(audioPlayback.volume * 100)}
            disabled={audioPlayback.muted}
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
            checked={videoSettings?.clipboard_sync_enabled ?? false}
            disabled={!videoSettings}
            loading={videoSettingsBusy}
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
          <span>{t("update.automatic")}</span>
          <Switch checked={automatic} onChange={setAutomatic} />
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
        <label><span>{t("settings.version")}</span><Typography.Text code>{version}</Typography.Text></label>
        <label><span>{t("settings.repository")}</span><Button icon={<GithubOutlined />} onClick={() => void openRepository()}>{t("settings.openGithub")}</Button></label>
      </div>
    </section>
  );
}
