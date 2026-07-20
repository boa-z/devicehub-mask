import { Select, Switch, Typography } from "antd";
import { useTranslation } from "react-i18next";
import { normalizeLanguage, type SupportedLanguage } from "../i18n";
import { UpdateButton } from "./UpdateButton";

type Props = {
  alwaysOnTop: boolean;
  fullscreen: boolean;
  inspectorVisible: boolean;
  onAlwaysOnTopChange: () => void;
  onFullscreenChange: () => void;
  onInspectorVisibleChange: (visible: boolean) => void;
};

export function SettingsPage({
  alwaysOnTop,
  fullscreen,
  inspectorVisible,
  onAlwaysOnTopChange,
  onFullscreenChange,
  onInspectorVisibleChange,
}: Props) {
  const { t, i18n } = useTranslation();
  const language = normalizeLanguage(i18n.resolvedLanguage ?? i18n.language);

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
        <Typography.Text type="secondary">{t("settings.languageSystemHint")}</Typography.Text>
      </div>
      <div className="settings-section">
        <Typography.Title level={5}>{t("settings.window")}</Typography.Title>
        <label><span>{t("settings.alwaysOnTop")}</span><Switch checked={alwaysOnTop} onChange={onAlwaysOnTopChange} /></label>
        <label><span>{t("settings.fullscreen")}</span><Switch checked={fullscreen} onChange={onFullscreenChange} /></label>
        <label><span>{t("settings.inspector")}</span><Switch checked={inspectorVisible} onChange={onInspectorVisibleChange} /></label>
      </div>
      <div className="settings-section">
        <Typography.Title level={5}>{t("settings.updates")}</Typography.Title>
        <UpdateButton />
      </div>
    </section>
  );
}
