import {
  CheckOutlined,
  CopyOutlined,
  DeleteOutlined,
  DownloadOutlined,
  EditOutlined,
  LinkOutlined,
  PlusOutlined,
  SaveOutlined,
  UploadOutlined,
} from "@ant-design/icons";
import { Button, Dropdown, Input, Modal, Select, Space, Tag, Tooltip, message } from "antd";
import { useRef, useState, type ChangeEvent } from "react";
import { useTranslation } from "react-i18next";
import { exportScrcpyMaskConfig, importScrcpyMaskConfig } from "../scrcpyCompat";
import { defaultHardwareBindings, type Profile } from "../types";

type DialogKind = "create" | "duplicate" | "rename";

type Props = {
  profile: Profile;
  profiles: string[];
  activeProfile: string;
  bindingConflicts: string[];
  frameSize: { width: number; height: number };
  onLoad: (name: string) => Promise<void>;
  onSave: () => Promise<void>;
  onActivate: () => Promise<void>;
  onCreate: (name: string) => Promise<void>;
  onDuplicate: (name: string) => Promise<void>;
  onRename: (name: string) => Promise<void>;
  onDelete: () => Promise<void>;
  onBundleIdentifiersChange: (bundleIdentifiers: string[]) => void;
  onImport: (profile: Profile, imported: number, skipped: number) => Promise<void>;
};

function profileName(value: string) {
  const name = value.trim().replace(/\.json$/i, "");
  return /^[A-Za-z0-9_-]{1,80}$/.test(name) ? name : undefined;
}

function downloadJson(name: string, value: unknown) {
  const blob = new Blob([JSON.stringify(value, null, 2)], { type: "application/json" });
  const url = URL.createObjectURL(blob);
  const link = document.createElement("a");
  link.href = url;
  link.download = name;
  link.click();
  URL.revokeObjectURL(url);
}

export function ProfileManager({
  profile,
  profiles,
  activeProfile,
  bindingConflicts,
  frameSize,
  onLoad,
  onSave,
  onActivate,
  onCreate,
  onDuplicate,
  onRename,
  onDelete,
  onBundleIdentifiersChange,
  onImport,
}: Props) {
  const { t } = useTranslation();
  const fileRef = useRef<HTMLInputElement>(null);
  const [dialog, setDialog] = useState<DialogKind | null>(null);
  const [nextName, setNextName] = useState("");
  const [appDialog, setAppDialog] = useState(false);
  const [nextBundleIdentifiers, setNextBundleIdentifiers] = useState<string[]>([]);
  const hasBindingConflict = profile.bundleIdentifiers.some((bundleId) => bindingConflicts.includes(bundleId));

  const openDialog = (kind: DialogKind) => {
    setDialog(kind);
    setNextName(kind === "rename" ? profile.name : kind === "duplicate" ? `${profile.name}-copy` : "");
  };
  const submitDialog = async () => {
    const name = profileName(nextName);
    if (!name) {
      void message.error(t("profile.invalidName"));
      return;
    }
    if (profiles.includes(name) && !(dialog === "rename" && name === profile.name)) {
      void message.error(t("profile.duplicateName"));
      return;
    }
    try {
      if (dialog === "create") await onCreate(name);
      if (dialog === "duplicate") await onDuplicate(name);
      if (dialog === "rename") await onRename(name);
      setDialog(null);
    } catch (error) {
      void message.error(String(error));
    }
  };
  const confirmDelete = () => Modal.confirm({
    title: t("profile.deleteTitle", { name: profile.name }),
    content: t("profile.deleteWarning"),
    okText: t("profile.delete"),
    okButtonProps: { danger: true },
    cancelText: t("common.cancel"),
    onOk: (close) => {
      void onDelete()
        .then(() => close())
        .catch((error) => message.error(String(error)));
    },
  });
  const importFile = async (event: ChangeEvent<HTMLInputElement>) => {
    const file = event.target.files?.[0];
    event.target.value = "";
    if (!file) return;
    try {
      const value = JSON.parse(await file.text()) as unknown;
      const importedName = file.name
        .replace(/(?:\.scrcpy-mask)?\.json$/i, "")
        .replace(/[^A-Za-z0-9_-]+/g, "-")
        .slice(0, 80);
      const baseName = profileName(importedName) ?? `import-${Date.now()}`;
      let name = baseName;
      let suffix = 2;
      while (profiles.includes(name)) {
        name = `${baseName}-import-${suffix}`;
        suffix += 1;
      }
      const native = value as Partial<Profile>;
      if (native.version === 1 && Array.isArray(native.mappings)) {
        await onImport({
          version: 1,
          name,
          mappings: native.mappings,
          hardwareBindings: { ...defaultHardwareBindings, ...native.hardwareBindings },
          bundleIdentifiers: Array.isArray(native.bundleIdentifiers) ? native.bundleIdentifiers : [],
        } as Profile, native.mappings.length, 0);
        return;
      }
      const result = importScrcpyMaskConfig(value, name, {
        invalidConfigMessage: t("profile.invalidScrcpy"),
        dpadLabel: t("mapping.dpad"),
      });
      await onImport(result.profile, result.imported, result.skipped);
    } catch (error) {
      void message.error(t("profile.importFailed", { error: String(error) }));
    }
  };

  return (
    <div className="profile-manager">
      <Select
        value={profile.name}
        options={profiles.map((name) => ({ value: name, label: name }))}
        onChange={(name) => void onLoad(name).catch((error) => message.error(String(error)))}
      />
      {activeProfile === profile.name && <Tag color="success">{t("profile.active")}</Tag>}
      <Space size={4}>
        <Tooltip title={t("profile.save")}><Button icon={<SaveOutlined />} onClick={() => void onSave()} /></Tooltip>
        <Tooltip title={t("profile.activate")}><Button disabled={activeProfile === profile.name} icon={<CheckOutlined />} onClick={() => void onActivate().catch((error) => message.error(String(error)))} /></Tooltip>
        <Tooltip title={t("profile.create")}><Button icon={<PlusOutlined />} onClick={() => openDialog("create")} /></Tooltip>
        <Tooltip title={t("profile.duplicate")}><Button icon={<CopyOutlined />} onClick={() => openDialog("duplicate")} /></Tooltip>
        <Tooltip title={t("profile.rename")}><Button icon={<EditOutlined />} onClick={() => openDialog("rename")} /></Tooltip>
        <Tooltip title={t("profile.delete")}><Button danger disabled={activeProfile === profile.name} icon={<DeleteOutlined />} onClick={confirmDelete} /></Tooltip>
        <Tooltip title={t(hasBindingConflict ? "profile.appBindingConflict" : "profile.appBindings")}>
          <Button
            danger={hasBindingConflict}
            type={profile.bundleIdentifiers.length > 0 ? "primary" : "default"}
            icon={<LinkOutlined />}
            onClick={() => {
              setNextBundleIdentifiers(profile.bundleIdentifiers);
              setAppDialog(true);
            }}
          />
        </Tooltip>
        <Tooltip title={t("profile.importJson")}><Button icon={<UploadOutlined />} onClick={() => fileRef.current?.click()} /></Tooltip>
        <Dropdown
          menu={{
            items: [
              { key: "native", label: "DeviceHub Mask" },
              { key: "scrcpy", label: "scrcpy-mask" },
            ],
            onClick: ({ key }) => {
              if (key === "native") downloadJson(`${profile.name}.json`, profile);
              if (key === "scrcpy") downloadJson(
                `${profile.name}.scrcpy-mask.json`,
                exportScrcpyMaskConfig(profile, frameSize.width, frameSize.height),
              );
            },
          }}
        >
          <Tooltip title={t("profile.exportJson")}><Button icon={<DownloadOutlined />} /></Tooltip>
        </Dropdown>
      </Space>
      <input ref={fileRef} className="file-input" type="file" accept="application/json,.json" onChange={(event) => void importFile(event)} />
      <Modal
        open={dialog !== null}
        title={dialog === "create" ? t("profile.createTitle") : dialog === "duplicate" ? t("profile.duplicateTitle") : t("profile.renameTitle")}
        okText={t("common.confirm")}
        cancelText={t("common.cancel")}
        onOk={() => void submitDialog()}
        onCancel={() => setDialog(null)}
      >
        <Input value={nextName} onChange={(event) => setNextName(event.target.value)} autoFocus />
      </Modal>
      <Modal
        open={appDialog}
        title={t("profile.appBindingsTitle")}
        okText={t("common.confirm")}
        cancelText={t("common.cancel")}
        onOk={() => {
          const normalized = [...new Set(nextBundleIdentifiers.map((value) => value.trim()).filter(Boolean))];
          const invalid = normalized.some((value) => value.length > 255 || !value.includes(".") || !/^[A-Za-z0-9.-]+$/.test(value));
          if (invalid || normalized.length > 32) {
            void message.error(t("profile.invalidAppBindings"));
            return;
          }
          onBundleIdentifiersChange(normalized);
          setAppDialog(false);
        }}
        onCancel={() => setAppDialog(false)}
      >
        <Select
          mode="tags"
          className="profile-app-bindings"
          value={nextBundleIdentifiers}
          tokenSeparators={[",", " "]}
          placeholder={t("profile.appBindingsPlaceholder")}
          onChange={setNextBundleIdentifiers}
        />
        <p className="profile-app-bindings-hint">{t("profile.appBindingsHint")}</p>
      </Modal>
    </div>
  );
}
