import i18n from "i18next";
import { initReactI18next } from "react-i18next";
import { enUS } from "./locales/en-US";
import { zhCN } from "./locales/zh-CN";

export const supportedLanguages = ["zh-CN", "en-US"] as const;
export type SupportedLanguage = (typeof supportedLanguages)[number];
export const localeStorageKey = "devicehub-mask.locale";

export function normalizeLanguage(language?: string | null): SupportedLanguage {
  return language?.toLowerCase().startsWith("zh") ? "zh-CN" : "en-US";
}

function initialLanguage() {
  if (typeof window === "undefined") return "en-US";
  try {
    return normalizeLanguage(localStorage.getItem(localeStorageKey) ?? navigator.language);
  } catch {
    return "en-US";
  }
}

void i18n
  .use(initReactI18next)
  .init({
    resources: {
      "en-US": { translation: enUS },
      "zh-CN": { translation: zhCN },
    },
    lng: initialLanguage(),
    fallbackLng: "en-US",
    supportedLngs: supportedLanguages,
    interpolation: { escapeValue: false },
  });

function applyLanguage(language: string) {
  const normalized = normalizeLanguage(language);
  if (typeof document === "undefined") return;
  document.documentElement.lang = normalized;
  try {
    localStorage.setItem(localeStorageKey, normalized);
  } catch {
    // The app remains usable when WebView storage is unavailable.
  }
}

applyLanguage(i18n.resolvedLanguage ?? i18n.language);
i18n.on("languageChanged", applyLanguage);

export default i18n;
