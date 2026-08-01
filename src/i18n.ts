import i18n from "i18next";
import { initReactI18next } from "react-i18next";
import enUS from "./locales/en-US.json";

export const supportedLanguages = ["zh-CN", "en-US"] as const;
export type SupportedLanguage = (typeof supportedLanguages)[number];
export const localeStorageKey = "devicehub-mask.locale";

export type LocaleResource<T> = {
  [Key in keyof T]: T[Key] extends object ? LocaleResource<T[Key]> : string;
};

type TranslationResource = LocaleResource<typeof enUS>;

const localeLoaders: Record<SupportedLanguage, () => Promise<TranslationResource>> = {
  "en-US": async () => enUS,
  "zh-CN": async () => (await import("./locales/zh-CN.json")).default,
};

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

i18n.on("languageChanged", applyLanguage);

export async function loadLanguage(language: SupportedLanguage) {
  const normalized = normalizeLanguage(language);
  if (!i18n.hasResourceBundle(normalized, "translation")) {
    const resource = await localeLoaders[normalized]();
    i18n.addResourceBundle(normalized, "translation", resource, true, true);
  }
  return normalized;
}

export async function changeLanguage(language: SupportedLanguage) {
  const normalized = await loadLanguage(language);
  await i18n.changeLanguage(normalized);
}

async function initializeI18n() {
  const language = initialLanguage();
  await i18n.use(initReactI18next).init({
    resources: {
      "en-US": { translation: enUS },
    },
    lng: "en-US",
    fallbackLng: "en-US",
    supportedLngs: supportedLanguages,
    interpolation: { escapeValue: false },
  });

  applyLanguage("en-US");
  if (language === "en-US") return;

  try {
    await changeLanguage(language);
  } catch (error) {
    console.warn(`Unable to load ${language} locale; using English instead.`, error);
  }
}

export const i18nReady = initializeI18n();

export default i18n;
