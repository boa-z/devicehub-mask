import { describe, expect, it } from "vitest";
import i18n, { changeLanguage, i18nReady, normalizeLanguage, supportedLanguages } from "./i18n";
import enUS from "./locales/en-US.json";
import zhCN from "./locales/zh-CN.json";

function keys(value: object, prefix = ""): string[] {
  return Object.entries(value).flatMap(([key, child]) => {
    const path = prefix ? `${prefix}.${key}` : key;
    return child !== null && typeof child === "object" ? keys(child, path) : [path];
  });
}

describe("localization", () => {
  it("keeps locale resource keys in sync", () => {
    expect(keys(zhCN)).toEqual(keys(enUS));
  });

  it("normalizes browser language variants", () => {
    expect(normalizeLanguage("zh-TW")).toBe("zh-CN");
    expect(normalizeLanguage("en-GB")).toBe("en-US");
    expect(normalizeLanguage(undefined)).toBe("en-US");
    expect(supportedLanguages).toEqual(["zh-CN", "en-US"]);
  });

  it("loads target locales before switching languages", async () => {
    await i18nReady;
    await changeLanguage("zh-CN");
    expect(i18n.t("common.confirm")).toBe("确认");
    await changeLanguage("en-US");
    expect(i18n.t("common.confirm")).toBe("Confirm");
  });
});
