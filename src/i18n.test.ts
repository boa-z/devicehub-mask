import { describe, expect, it } from "vitest";
import { normalizeLanguage, supportedLanguages } from "./i18n";
import { enUS } from "./locales/en-US";
import { zhCN } from "./locales/zh-CN";

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
});
