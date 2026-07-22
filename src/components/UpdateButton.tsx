import { CloudDownloadOutlined } from "@ant-design/icons";
import { isTauri } from "@tauri-apps/api/core";
import { relaunch } from "@tauri-apps/plugin-process";
import { check, type Update } from "@tauri-apps/plugin-updater";
import { Button, Modal, message } from "antd";
import { useCallback, useEffect, useRef, useState, type ReactNode } from "react";
import { useTranslation } from "react-i18next";
import { UpdateContext, type UpdateContextValue, useUpdates } from "../updateContext";
import { readAutomaticUpdatePreference, writeAutomaticUpdatePreference } from "../updatePreferences";
import { logFrontend } from "../diagnostics";

const progressMessageKey = "app-update-progress";

export function UpdateProvider({ children }: { children: ReactNode }) {
  const { t } = useTranslation();
  const translateRef = useRef(t);
  translateRef.current = t;
  const [automatic, setAutomaticState] = useState(readAutomaticUpdatePreference);
  const [checking, setChecking] = useState(false);
  const checkingRef = useRef(false);

  const setAutomatic = useCallback((enabled: boolean) => {
    setAutomaticState(enabled);
    writeAutomaticUpdatePreference(enabled);
  }, []);

  const install = useCallback(async (update: Update) => {
    let downloaded = 0;
    let total: number | undefined;
    await update.downloadAndInstall((event) => {
      if (event.event === "Started") {
        total = event.data.contentLength;
        void message.loading({ content: translateRef.current("update.downloading", { progress: "" }), key: progressMessageKey, duration: 0 });
      } else if (event.event === "Progress") {
        downloaded += event.data.chunkLength;
        const progress = total ? ` ${Math.min(100, Math.round(downloaded / total * 100))}%` : "";
        void message.loading({ content: translateRef.current("update.downloading", { progress }), key: progressMessageKey, duration: 0 });
      } else {
        void message.loading({ content: translateRef.current("update.installing"), key: progressMessageKey, duration: 0 });
      }
    });
    void message.success({ content: translateRef.current("update.restarting"), key: progressMessageKey, duration: 2 });
    await relaunch();
  }, []);

  const checkForUpdate = useCallback(async (manual: boolean) => {
    if (!isTauri() || checkingRef.current) return;
    checkingRef.current = true;
    setChecking(true);
    try {
      const update = await check({ timeout: 15_000 });
      if (!update) {
        if (manual) void message.success(translateRef.current("update.latest"));
        return;
      }
      Modal.confirm({
        title: translateRef.current("update.available", { version: update.version }),
        content: update.body || translateRef.current("update.prompt", { current: update.currentVersion }),
        okText: translateRef.current("update.installAndRestart"),
        cancelText: translateRef.current("update.later"),
        onOk: () => install(update),
        onCancel: () => update.close(),
      });
    } catch (error) {
      logFrontend("warn", "updater", "check_for_update", error);
      if (manual) void message.error(translateRef.current("update.failed", { error: String(error) }));
    } finally {
      checkingRef.current = false;
      setChecking(false);
    }
  }, [install]);

  useEffect(() => {
    if (!automatic || !isTauri()) return;
    const timer = window.setTimeout(() => void checkForUpdate(false), 3_000);
    return () => clearTimeout(timer);
  }, [automatic, checkForUpdate]);

  const value: UpdateContextValue = {
    automatic,
    checking,
    setAutomatic,
    checkNow: () => void checkForUpdate(true),
  };

  return <UpdateContext.Provider value={value}>{children}</UpdateContext.Provider>;
}

export function UpdateButton() {
  const { t } = useTranslation();
  const { checking, checkNow } = useUpdates();

  return (
    <Button
      icon={<CloudDownloadOutlined />}
      loading={checking}
      onClick={checkNow}
    >
      {t("update.checkNow")}
    </Button>
  );
}
