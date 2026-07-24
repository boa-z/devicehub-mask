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
import { Button, Dropdown, Input, Modal, Select, Space, Tag, Tooltip, Typography } from "antd";
import { useRef, useState, type ChangeEvent } from "react";
import { useTranslation } from "react-i18next";
import { importMappingFile, mappingImportSource, mappingImportSources, uniqueImportedProfileName, type MappingImportSourceId } from "../mappingImport";
import { exportScrcpyMaskConfig } from "../scrcpyCompat";
import { showErrorMessage } from "../errorMessage";
import type { Profile } from "../types";

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
  const name = value.trim().replace(/\.(?:playmap|json)$/i, "");
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
  const [importDialog, setImportDialog] = useState(false);
  const [importSource, setImportSource] = useState<MappingImportSourceId>("devicehub-mask");
  const [selectedImportFile, setSelectedImportFile] = useState<File | null>(null);
  const [importBusy, setImportBusy] = useState(false);
  const hasBindingConflict = profile.bundleIdentifiers.some((bundleId) => bindingConflicts.includes(bundleId));

  const openDialog = (kind: DialogKind) => {
    setDialog(kind);
    setNextName(kind === "rename" ? profile.name : kind === "duplicate" ? `${profile.name}-copy` : "");
  };
  const submitDialog = async () => {
    const name = profileName(nextName);
    if (!name) {
      void showErrorMessage(t("profile.invalidName"));
      return;
    }
    if (profiles.includes(name) && !(dialog === "rename" && name === profile.name)) {
      void showErrorMessage(t("profile.duplicateName"));
      return;
    }
    try {
      if (dialog === "create") await onCreate(name);
      if (dialog === "duplicate") await onDuplicate(name);
      if (dialog === "rename") await onRename(name);
      setDialog(null);
    } catch (error) {
      void showErrorMessage(error);
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
        .catch((error) => showErrorMessage(error));
    },
  });
  const chooseImportFile = (event: ChangeEvent<HTMLInputElement>) => {
    const file = event.target.files?.[0] ?? null;
    event.target.value = "";
    setSelectedImportFile(file);
  };
  const submitImport = async () => {
    if (!selectedImportFile) return;
    setImportBusy(true);
    try {
      const result = await importMappingFile(importSource, selectedImportFile, {
        profileName: uniqueImportedProfileName(selectedImportFile.name, profiles),
        frameSize,
        invalidMessages: {
          "devicehub-mask": t("profile.invalidNative"),
          "scrcpy-mask": t("profile.invalidScrcpy"),
          playcover: t("profile.invalidPlayCover"),
        },
        playCoverLabels: {
          button: t("profile.playCoverButton"),
          draggable: t("profile.playCoverDrag"),
          joystick: t("profile.playCoverJoystick"),
        },
        dpadLabel: t("mapping.dpad"),
      });
      await onImport(result.profile, result.imported, result.skipped);
      setImportDialog(false);
      setSelectedImportFile(null);
    } catch (error) {
      void showErrorMessage(t("profile.importFailed", { error: String(error) }));
    } finally {
      setImportBusy(false);
    }
  };

  return (
    <div className="profile-manager">
      <Select
        value={profile.name}
        options={profiles.map((name) => ({ value: name, label: name }))}
        onChange={(name) => void onLoad(name).catch((error) => showErrorMessage(error))}
      />
      {activeProfile === profile.name && <Tag color="success">{t("profile.active")}</Tag>}
      <Space size={4}>
        <Tooltip title={t("profile.save")}><Button icon={<SaveOutlined />} onClick={() => void onSave()} /></Tooltip>
        <Tooltip title={t("profile.activate")}><Button disabled={activeProfile === profile.name} icon={<CheckOutlined />} onClick={() => void onActivate().catch((error) => showErrorMessage(error))} /></Tooltip>
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
        <Tooltip title={t("profile.importConfig")}><Button icon={<UploadOutlined />} onClick={() => setImportDialog(true)} /></Tooltip>
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
      <input ref={fileRef} className="file-input" type="file" accept={mappingImportSource(importSource).accept} onChange={chooseImportFile} />
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
            void showErrorMessage(t("profile.invalidAppBindings"));
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
      <Modal
        open={importDialog}
        title={t("profile.importTitle")}
        okText={t("profile.importConfig")}
        cancelText={t("common.cancel")}
        okButtonProps={{ disabled: selectedImportFile === null }}
        confirmLoading={importBusy}
        onOk={() => void submitImport()}
        onCancel={() => {
          if (importBusy) return;
          setImportDialog(false);
          setSelectedImportFile(null);
        }}
      >
        <div className="profile-import-form">
          <label>
            <span>{t("profile.importSource")}</span>
            <Select
              value={importSource}
              options={mappingImportSources.map((source) => ({
                value: source.id,
                label: t(`profile.importSources.${source.id}`),
              }))}
              onChange={(source) => {
                setImportSource(source);
                setSelectedImportFile(null);
              }}
            />
          </label>
          <div className="profile-import-file">
            <Button icon={<UploadOutlined />} onClick={() => fileRef.current?.click()}>{t("profile.chooseImportFile")}</Button>
            <Typography.Text ellipsis={{ tooltip: selectedImportFile?.name }} type={selectedImportFile ? undefined : "secondary"}>
              {selectedImportFile?.name ?? t("profile.noImportFile")}
            </Typography.Text>
          </div>
          <Typography.Text type="secondary" className="profile-import-formats">
            {t("profile.importFormats", { formats: mappingImportSource(importSource).extensions.join(", ") })}
          </Typography.Text>
        </div>
      </Modal>
    </div>
  );
}
