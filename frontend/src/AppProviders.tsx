import { ConfigProvider, theme } from "antd";
import enUS from "antd/locale/en_US";
import zhCN from "antd/locale/zh_CN";
import { useTranslation } from "react-i18next";
import App from "./App";
import { normalizeLanguage } from "./i18n";

export function AppProviders() {
  const { i18n } = useTranslation();
  const language = normalizeLanguage(i18n.resolvedLanguage ?? i18n.language);

  return (
    <ConfigProvider
      locale={language === "zh-CN" ? zhCN : enUS}
      theme={{
        algorithm: theme.darkAlgorithm,
        token: { colorPrimary: "#42b883", borderRadius: 6, fontFamily: "var(--system-font)" },
      }}
    >
      <App />
    </ConfigProvider>
  );
}
