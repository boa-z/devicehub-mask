import { DeleteOutlined, PlusOutlined } from "@ant-design/icons";
import { Button, Divider, Dropdown, Input, InputNumber, Select, Typography } from "antd";
import { useTranslation } from "react-i18next";
import { mappingContactIds, mappingLabel, scrcpyMappingTypes, hardwareButtons, type ButtonBinding, type DirectionBinding, type HardwareBindings, type HardwareButtonName, type Mapping, type ScrcpyMappingType } from "../types";

type Props = { mappings: Mapping[]; selectedId: string | null; onSelect: (id: string) => void; onChange: (mapping: Mapping) => void; onAdd: (type: ScrcpyMappingType) => void; onDelete: (id: string) => void; hardwareBindings: HardwareBindings; onHardwareBindingChange: (name: HardwareButtonName, key: string) => void };

function KeyInput({ value, onChange }: { value: string; onChange: (value: string) => void }) {
  const { t } = useTranslation();
  return <Input value={value} readOnly onKeyDown={(event) => { event.preventDefault(); event.stopPropagation(); onChange(event.code === "Backspace" || event.code === "Delete" ? "" : event.code); }} placeholder={t("mapping.pressKey")} />;
}

function BindingInput({ value, onChange }: { value: ButtonBinding; onChange: (value: ButtonBinding) => void }) {
  return <KeyInput value={value.join(" + ")} onChange={(key) => onChange(key ? [key] : [])} />;
}

function DirectionInputs({ value, onChange }: { value: DirectionBinding; onChange: (value: DirectionBinding) => void }) {
  if (value.type !== "Button") return <Typography.Text type="secondary">Joystick bindings are preserved from imported configurations.</Typography.Text>;
  return <>{(["up", "down", "left", "right"] as const).map((direction) => <label key={direction}>{direction.toUpperCase()}<BindingInput value={value[direction]} onChange={(binding) => onChange({ ...value, [direction]: binding })} /></label>)}</>;
}

export function MappingInspector({ mappings, selectedId, onSelect, onChange, onAdd, onDelete, hardwareBindings, onHardwareBindingChange }: Props) {
  const { t } = useTranslation();
  const selected = mappings.find((mapping) => mapping.id === selectedId) ?? null;
  const patch = (values: object) => selected && onChange({ ...selected, ...values } as Mapping);
  const pointerId = selected && ("contactId" in selected ? selected.contactId : "pointer_id" in selected ? selected.pointer_id : null);
  const binding = selected && (selected.type === "touch" ? [selected.key] : "bind" in selected && Array.isArray(selected.bind) ? selected.bind : null);
  const numberField = (label: string, key: string, value: number, min = 0, step = 1) => <label>{label}<InputNumber min={min} step={step} value={value} onChange={(next) => next !== null && patch({ [key]: next })} /></label>;

  return <aside className="inspector">
    <div className="inspector-title"><Typography.Title level={5}>{t("mapping.title")}</Typography.Title><Dropdown menu={{ items: scrcpyMappingTypes.map((type) => ({ key: type, label: t(`mapping.types.${type}`) })), onClick: ({ key }) => onAdd(key as ScrcpyMappingType) }}><Button icon={<PlusOutlined />} title={t("mapping.add")} /></Dropdown></div>
    <div className="mapping-list">{mappings.map((mapping) => <button key={mapping.id} className={mapping.id === selectedId ? "selected" : ""} onClick={() => onSelect(mapping.id)}><span className={`mapping-dot ${mapping.type}`} /><strong>{mappingLabel(mapping)}</strong><small>{mappingContactIds(mapping).length ? mappingContactIds(mapping).join(", ") : "-"}</small></button>)}</div>
    <Divider />
    {selected ? <div className="fields">
      <label>{t("mapping.name")}<Input value={mappingLabel(selected)} onChange={(event) => patch("label" in selected ? { label: event.target.value } : { note: event.target.value })} /></label>
      <label>{t("mapping.type")}<Select value={selected.type} disabled options={[{ value: selected.type, label: selected.type === "touch" ? t("mapping.types.SingleTap") : selected.type === "dpad" ? t("mapping.types.DirectionPad") : t(`mapping.types.${selected.type}`) }]} /></label>
      {pointerId !== null && <label>{t("mapping.contactId")}<InputNumber min={0} max={4} value={pointerId} onChange={(value) => value !== null && patch("contactId" in selected ? { contactId: value } : { pointer_id: value })} /></label>}
      {binding && <label>{t("mapping.keyboardBinding")}<BindingInput value={binding} onChange={(value) => patch(selected.type === "touch" ? { key: value[0] ?? "" } : { bind: value })} /></label>}
      {selected.type === "dpad" && <><DirectionInputs value={{ type: "Button", up: [selected.keys.up], down: [selected.keys.down], left: [selected.keys.left], right: [selected.keys.right] }} onChange={(value) => value.type === "Button" && patch({ keys: { up: value.up[0] ?? "", down: value.down[0] ?? "", left: value.left[0] ?? "", right: value.right[0] ?? "" } })} />{numberField(t("mapping.radius"), "radius", selected.radius, 0.01, 0.01)}</>}
      {selected.type === "DirectionPad" && <><DirectionInputs value={selected.bind} onChange={(bind) => patch({ bind })} />{numberField(t("mapping.offsetX"), "max_offset_x", selected.max_offset_x)}{numberField(t("mapping.offsetY"), "max_offset_y", selected.max_offset_y)}</>}
      {selected.type === "PadCastSpell" && <><DirectionInputs value={selected.pad_bind} onChange={(pad_bind) => patch({ pad_bind })} />{numberField(t("mapping.dragRadius"), "drag_radius", selected.drag_radius)}</>}
      {(selected.type === "SingleTap" || selected.type === "RepeatTap") && numberField(t("mapping.duration"), "duration", selected.duration)}
      {selected.type === "RepeatTap" && numberField(t("mapping.interval"), "interval", selected.interval, 1)}
      {selected.type === "Swipe" && numberField(t("mapping.duration"), "duration", selected.duration)}
      {(selected.type === "MouseCastSpell" || selected.type === "PadCastSpell") && <label>{t("mapping.releaseMode")}<Select value={selected.release_mode} options={(selected.type === "MouseCastSpell" ? ["OnPress", "OnRelease", "OnSecondPress"] : ["OnRelease", "OnSecondPress"]).map((value) => ({ value }))} onChange={(release_mode) => patch({ release_mode })} /></label>}
      {selected.type === "MouseCastSpell" && <>{numberField(t("mapping.castRadius"), "cast_radius", selected.cast_radius)}{numberField(t("mapping.dragRadius"), "drag_radius", selected.drag_radius)}</>}
      {(selected.type === "Observation" || selected.type === "Fps" || selected.type === "Fire") && <>{numberField(t("mapping.sensitivityX"), "sensitivity_x", selected.sensitivity_x, 0, 0.1)}{numberField(t("mapping.sensitivityY"), "sensitivity_y", selected.sensitivity_y, 0, 0.1)}</>}
      {selected.type === "Script" && <><label>{t("mapping.pressedScript")}<Input.TextArea rows={3} value={selected.pressed_script} onChange={(event) => patch({ pressed_script: event.target.value })} /></label><label>{t("mapping.heldScript")}<Input.TextArea rows={3} value={selected.held_script} onChange={(event) => patch({ held_script: event.target.value })} /></label><label>{t("mapping.releasedScript")}<Input.TextArea rows={3} value={selected.released_script} onChange={(event) => patch({ released_script: event.target.value })} /></label>{numberField(t("mapping.interval"), "interval", selected.interval, 1)}</>}
      {(selected.type === "MultipleTap" || selected.type === "Swipe") && <Typography.Text type="secondary">{t("mapping.sequenceSummary", { count: selected.type === "MultipleTap" ? selected.items.length : selected.positions.length })}</Typography.Text>}
      <Button danger icon={<DeleteOutlined />} onClick={() => onDelete(selected.id)}>{t("mapping.delete")}</Button>
    </div> : <Typography.Text type="secondary">{t("mapping.selectHint")}</Typography.Text>}
    <Divider /><Typography.Title level={5}>{t("mapping.hardwareShortcuts")}</Typography.Title><div className="hardware-binding-list">{hardwareButtons.map((button) => <label key={button.name}><span>{t(`hardware.${button.name}`)}</span><KeyInput value={hardwareBindings[button.name]} onChange={(key) => onHardwareBindingChange(button.name, key)} /></label>)}</div>
  </aside>;
}
