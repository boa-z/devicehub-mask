import { DeleteOutlined, PlusOutlined } from "@ant-design/icons";
import { Button, Divider, Input, InputNumber, Segmented, Space, Typography } from "antd";
import { useTranslation } from "react-i18next";
import { hardwareButtons, type DpadMapping, type HardwareBindings, type HardwareButtonName, type Mapping, type TouchMapping } from "../types";

type Props = {
  mappings: Mapping[];
  selectedId: string | null;
  onSelect: (id: string) => void;
  onChange: (mapping: Mapping) => void;
  onAdd: (type: "touch" | "dpad") => void;
  onDelete: (id: string) => void;
  hardwareBindings: HardwareBindings;
  onHardwareBindingChange: (name: HardwareButtonName, key: string) => void;
};

function KeyInput({ value, onChange }: { value: string; onChange: (value: string) => void }) {
  const { t } = useTranslation();
  return (
    <Input
      value={value}
      readOnly
      onKeyDown={(event) => {
        event.preventDefault();
        event.stopPropagation();
        onChange(event.code === "Backspace" || event.code === "Delete" ? "" : event.code);
      }}
      placeholder={t("mapping.pressKey")}
    />
  );
}

export function MappingInspector({ mappings, selectedId, onSelect, onChange, onAdd, onDelete, hardwareBindings, onHardwareBindingChange }: Props) {
  const { t } = useTranslation();
  const selected = mappings.find((mapping) => mapping.id === selectedId) ?? null;
  const patch = (values: Partial<Mapping>) => selected && onChange({ ...selected, ...values } as Mapping);

  return (
    <aside className="inspector">
      <div className="inspector-title">
        <Typography.Title level={5}>{t("mapping.title")}</Typography.Title>
        <Space.Compact>
          <Button icon={<PlusOutlined />} title={t("mapping.addTouch")} onClick={() => onAdd("touch")} />
          <Button onClick={() => onAdd("dpad")}>{t("mapping.dpad")}</Button>
        </Space.Compact>
      </div>
      <div className="mapping-list">
        {mappings.map((mapping) => (
          <button key={mapping.id} className={mapping.id === selectedId ? "selected" : ""} onClick={() => onSelect(mapping.id)}>
            <span className={`mapping-dot ${mapping.type}`} />
            <strong>{mapping.label}</strong>
            <small>{t("mapping.contact", { id: mapping.contactId })}</small>
          </button>
        ))}
      </div>
      <Divider />
      {selected ? (
        <div className="fields">
          <label>{t("mapping.name")}<Input value={selected.label} onChange={(event) => patch({ label: event.target.value })} /></label>
          <label>{t("mapping.contactId")}<InputNumber min={0} max={4} value={selected.contactId} onChange={(value) => value !== null && patch({ contactId: value })} /></label>
          <label>{t("mapping.type")}<Segmented block value={selected.type} options={[{ label: t("mapping.touch"), value: "touch" }, { label: t("mapping.dpad"), value: "dpad" }]} disabled /></label>
          {selected.type === "touch" ? (
            <label>{t("mapping.keyboardBinding")}<KeyInput value={selected.key} onChange={(key) => onChange({ ...selected, key } as TouchMapping)} /></label>
          ) : (
            <>
              {(["up", "down", "left", "right"] as const).map((direction) => (
                <label key={direction}>{direction.toUpperCase()}<KeyInput value={selected.keys[direction]} onChange={(key) => onChange({ ...selected, keys: { ...selected.keys, [direction]: key } } as DpadMapping)} /></label>
              ))}
              <label>{t("mapping.radius")}<InputNumber min={0.02} max={0.3} step={0.01} value={selected.radius} onChange={(radius) => radius !== null && onChange({ ...selected, radius } as DpadMapping)} /></label>
            </>
          )}
          <Button danger icon={<DeleteOutlined />} onClick={() => onDelete(selected.id)}>{t("mapping.delete")}</Button>
        </div>
      ) : <Typography.Text type="secondary">{t("mapping.selectHint")}</Typography.Text>}
      <Divider />
      <Typography.Title level={5}>{t("mapping.hardwareShortcuts")}</Typography.Title>
      <div className="hardware-binding-list">
        {hardwareButtons.map((button) => (
          <label key={button.name}>
            <span>{t(`hardware.${button.name}`)}</span>
            <KeyInput
              value={hardwareBindings[button.name]}
              onChange={(key) => onHardwareBindingChange(button.name, key)}
            />
          </label>
        ))}
      </div>
    </aside>
  );
}
