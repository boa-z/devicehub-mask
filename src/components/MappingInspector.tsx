import {
  CopyOutlined,
  DeleteOutlined,
  PlusOutlined,
  SearchOutlined,
} from "@ant-design/icons";
import { Button, Dropdown, Empty, Input, InputNumber, Segmented, Select, Space, Tag, Tooltip, Typography } from "antd";
import { useMemo, useState } from "react";
import { useTranslation } from "react-i18next";
import {
  hardwareButtons,
  keyboardBindingLabel,
  mappingBindingLabel,
  mappingContactIds,
  mappingLabel,
  mappingPosition,
  scrcpyMappingTypes,
  type ButtonBinding,
  type DirectionBinding,
  type HardwareBindings,
  type HardwareButtonName,
  type Mapping,
  type Position,
  type ScrcpyMappingType,
} from "../types";

type Props = {
  mappings: Mapping[];
  selectedId: string | null;
  onSelect: (id: string) => void;
  onChange: (mapping: Mapping) => void;
  onAdd: (type: ScrcpyMappingType) => void;
  onDuplicate: (id: string) => void;
  onDelete: (id: string) => void;
  hardwareBindings: HardwareBindings;
  onHardwareBindingChange: (name: HardwareButtonName, key: string) => void;
};

function modifierBinding(event: React.KeyboardEvent<HTMLInputElement>) {
  const keys: string[] = [];
  const add = (key: string) => { if (!keys.includes(key)) keys.push(key); };
  if (event.ctrlKey) add(event.code.startsWith("Control") ? event.code : "ControlLeft");
  if (event.shiftKey) add(event.code.startsWith("Shift") ? event.code : "ShiftLeft");
  if (event.altKey) add(event.code.startsWith("Alt") ? event.code : "AltLeft");
  if (event.metaKey) add(event.code.startsWith("Meta") ? event.code : "MetaLeft");
  if (!/^(Control|Shift|Alt|Meta)(Left|Right)$/.test(event.code)) add(event.code);
  return keys;
}

function KeyInput({ value, onChange }: { value: string; onChange: (value: string) => void }) {
  const { t } = useTranslation();
  return (
    <Input
      value={value ? keyboardBindingLabel(value) : ""}
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

function BindingInput({ value, onChange }: { value: ButtonBinding; onChange: (value: ButtonBinding) => void }) {
  const { t } = useTranslation();
  return (
    <Input
      value={value.map(keyboardBindingLabel).join(" + ")}
      readOnly
      onKeyDown={(event) => {
        event.preventDefault();
        event.stopPropagation();
        onChange(event.code === "Backspace" || event.code === "Delete" ? [] : modifierBinding(event));
      }}
      placeholder={t("mapping.pressKey")}
    />
  );
}

function PositionInput({ value, onChange }: { value: Position; onChange: (value: Position) => void }) {
  return (
    <Space.Compact block>
      <InputNumber
        aria-label="X"
        prefix="X"
        suffix="%"
        min={0}
        max={100}
        step={0.1}
        value={Number((value.x * 100).toFixed(2))}
        onChange={(x) => x !== null && onChange({ ...value, x: x / 100 })}
      />
      <InputNumber
        aria-label="Y"
        prefix="Y"
        suffix="%"
        min={0}
        max={100}
        step={0.1}
        value={Number((value.y * 100).toFixed(2))}
        onChange={(y) => y !== null && onChange({ ...value, y: y / 100 })}
      />
    </Space.Compact>
  );
}

function DirectionInputs({ value, onChange }: { value: DirectionBinding; onChange: (value: DirectionBinding) => void }) {
  const { t } = useTranslation();
  if (value.type !== "Button") {
    return <Typography.Text type="secondary">{t("mapping.joystickPreserved")}</Typography.Text>;
  }
  return (
    <div className="mapping-direction-grid mapping-wide-field">
      {(["up", "left", "down", "right"] as const).map((direction) => (
        <label key={direction}>
          <span>{t(`mapping.directions.${direction}`)}</span>
          <BindingInput value={value[direction]} onChange={(binding) => onChange({ ...value, [direction]: binding })} />
        </label>
      ))}
    </div>
  );
}

function FieldSection({ title, children }: React.PropsWithChildren<{ title: string }>) {
  return (
    <section className="mapping-field-section">
      <Typography.Text strong>{title}</Typography.Text>
      <div className="mapping-field-grid">{children}</div>
    </section>
  );
}

export function MappingInspector({
  mappings,
  selectedId,
  onSelect,
  onChange,
  onAdd,
  onDuplicate,
  onDelete,
  hardwareBindings,
  onHardwareBindingChange,
}: Props) {
  const { t } = useTranslation();
  const [panel, setPanel] = useState<"mappings" | "hardware">("mappings");
  const [query, setQuery] = useState("");
  const selected = mappings.find((mapping) => mapping.id === selectedId) ?? null;
  const visibleMappings = useMemo(() => {
    const normalized = query.trim().toLocaleLowerCase();
    if (!normalized) return mappings;
    return mappings.filter((mapping) => [mappingLabel(mapping), mapping.type, mappingBindingLabel(mapping)]
      .some((value) => value?.toLocaleLowerCase().includes(normalized)));
  }, [mappings, query]);
  const patch = (values: object) => selected && onChange({ ...selected, ...values } as Mapping);
  const pointerId = selected && ("contactId" in selected ? selected.contactId : "pointer_id" in selected ? selected.pointer_id : null);
  const binding = selected && (selected.type === "touch" ? [selected.key] : "bind" in selected && Array.isArray(selected.bind) ? selected.bind : null);
  const primaryPosition = selected ? mappingPosition(selected) : null;
  const setPrimaryPosition = (position: Position) => selected && patch("position" in selected ? { position } : { x: position.x, y: position.y });
  const sequenceItems: { position: Position; duration?: number; wait?: number }[] = selected?.type === "MultipleTap"
    ? selected.items.map((item) => ({ position: item.position, duration: item.duration, wait: item.wait }))
    : selected?.type === "Swipe"
      ? selected.positions.map((position) => ({ position }))
      : [];
  const hasBehaviorFields = selected !== null && [
    "dpad", "DirectionPad", "PadCastSpell", "SingleTap", "RepeatTap", "Swipe",
    "MouseCastSpell", "Observation", "Fps", "Fire",
  ].includes(selected.type);
  const numberField = (label: string, key: string, value: number, min = 0, step = 1) => (
    <label>
      <span>{label}</span>
      <InputNumber min={min} step={step} value={value} onChange={(next) => next !== null && patch({ [key]: next })} />
    </label>
  );
  const addMenu = {
    items: scrcpyMappingTypes.map((type) => ({ key: type, label: t(`mapping.types.${type}`) })),
    onClick: ({ key }: { key: string }) => onAdd(key as ScrcpyMappingType),
  };

  return (
    <aside className="inspector mapping-inspector">
      <div className="inspector-title">
        <div>
          <Typography.Title level={5}>{t("mapping.title")}</Typography.Title>
          <Tag>{mappings.length}</Tag>
        </div>
        <Dropdown menu={addMenu}>
          <Tooltip title={t("mapping.add")}><Button aria-label={t("mapping.add")} icon={<PlusOutlined />} /></Tooltip>
        </Dropdown>
      </div>
      <Segmented
        block
        value={panel}
        options={[
          { value: "mappings", label: t("mapping.controllers") },
          { value: "hardware", label: t("mapping.hardwareShortcuts") },
        ]}
        onChange={setPanel}
      />

      {panel === "hardware" ? (
        <div className="hardware-binding-list mapping-panel-scroll">
          {hardwareButtons.map((button) => (
            <label key={button.name}>
              <span>{t(`hardware.${button.name}`)}</span>
              <KeyInput value={hardwareBindings[button.name]} onChange={(key) => onHardwareBindingChange(button.name, key)} />
            </label>
          ))}
        </div>
      ) : (
        <div className="mapping-panel-scroll">
          <Input
            allowClear
            prefix={<SearchOutlined />}
            value={query}
            placeholder={t("mapping.search")}
            onChange={(event) => setQuery(event.target.value)}
          />
          <div className="mapping-list">
            {visibleMappings.map((mapping) => {
              const contacts = mappingContactIds(mapping);
              const bindingLabel = mappingBindingLabel(mapping);
              return (
                <button key={mapping.id} className={mapping.id === selectedId ? "selected" : ""} onClick={() => onSelect(mapping.id)}>
                  <span className={`mapping-dot ${mapping.type}`} />
                  <span className="mapping-list-copy">
                    <strong>{mappingLabel(mapping)}</strong>
                    <small>{t(`mapping.types.${mapping.type === "touch" ? "SingleTap" : mapping.type === "dpad" ? "DirectionPad" : mapping.type}`)}</small>
                  </span>
                  <span className="mapping-list-meta">
                    {bindingLabel && <Tag>{bindingLabel}</Tag>}
                    {contacts.length > 0 && <small>{contacts.join("/")}</small>}
                  </span>
                </button>
              );
            })}
          </div>

          {selected ? (
            <div className="mapping-fields">
              <div className="mapping-selection-header">
                <Typography.Text strong ellipsis={{ tooltip: mappingLabel(selected) }}>{mappingLabel(selected)}</Typography.Text>
                <Space size={4}>
                  <Tooltip title={t("mapping.duplicate")}><Button size="small" aria-label={t("mapping.duplicate")} icon={<CopyOutlined />} onClick={() => onDuplicate(selected.id)} /></Tooltip>
                  <Tooltip title={t("mapping.delete")}><Button danger size="small" aria-label={t("mapping.delete")} icon={<DeleteOutlined />} onClick={() => onDelete(selected.id)} /></Tooltip>
                </Space>
              </div>

              <FieldSection title={t("mapping.basic") }>
                <label><span>{t("mapping.name")}</span><Input value={mappingLabel(selected)} onChange={(event) => patch("label" in selected ? { label: event.target.value } : { note: event.target.value })} /></label>
                <label><span>{t("mapping.type")}</span><Select value={selected.type} disabled options={[{ value: selected.type, label: t(`mapping.types.${selected.type === "touch" ? "SingleTap" : selected.type === "dpad" ? "DirectionPad" : selected.type}`) }]} /></label>
                {primaryPosition && <label className="mapping-wide-field"><span>{t("mapping.position")}</span><PositionInput value={primaryPosition} onChange={setPrimaryPosition} /></label>}
                {pointerId !== null && <label><span>{t("mapping.contactId")}</span><InputNumber min={0} max={4} value={pointerId} onChange={(value) => value !== null && patch("contactId" in selected ? { contactId: value } : { pointer_id: value })} /></label>}
              </FieldSection>

              {(binding || selected.type === "dpad" || selected.type === "DirectionPad" || selected.type === "PadCastSpell") && (
                <FieldSection title={t("mapping.input") }>
                  {binding && <label className="mapping-wide-field"><span>{t("mapping.keyboardBinding")}</span><BindingInput value={binding} onChange={(value) => patch(selected.type === "touch" ? { key: value[0] ?? "" } : { bind: value })} /></label>}
                  {selected.type === "dpad" && <DirectionInputs value={{ type: "Button", up: [selected.keys.up], down: [selected.keys.down], left: [selected.keys.left], right: [selected.keys.right] }} onChange={(value) => value.type === "Button" && patch({ keys: { up: value.up[0] ?? "", down: value.down[0] ?? "", left: value.left[0] ?? "", right: value.right[0] ?? "" } })} />}
                  {selected.type === "DirectionPad" && <DirectionInputs value={selected.bind} onChange={(bind) => patch({ bind })} />}
                  {selected.type === "PadCastSpell" && <DirectionInputs value={selected.pad_bind} onChange={(pad_bind) => patch({ pad_bind })} />}
                </FieldSection>
              )}

              {hasBehaviorFields && <FieldSection title={t("mapping.behavior") }>
                {selected.type === "dpad" && numberField(t("mapping.radius"), "radius", selected.radius, 0.01, 0.01)}
                {selected.type === "DirectionPad" && <>{numberField(t("mapping.offsetX"), "max_offset_x", selected.max_offset_x)}{numberField(t("mapping.offsetY"), "max_offset_y", selected.max_offset_y)}</>}
                {selected.type === "PadCastSpell" && numberField(t("mapping.dragRadius"), "drag_radius", selected.drag_radius)}
                {(selected.type === "SingleTap" || selected.type === "RepeatTap") && numberField(t("mapping.duration"), "duration", selected.duration)}
                {selected.type === "RepeatTap" && numberField(t("mapping.interval"), "interval", selected.interval, 1)}
                {selected.type === "Swipe" && numberField(t("mapping.duration"), "duration", selected.duration)}
                {(selected.type === "MouseCastSpell" || selected.type === "PadCastSpell") && <label><span>{t("mapping.releaseMode")}</span><Select value={selected.release_mode} options={(selected.type === "MouseCastSpell" ? ["OnPress", "OnRelease", "OnSecondPress"] : ["OnRelease", "OnSecondPress"]).map((value) => ({ value }))} onChange={(release_mode) => patch({ release_mode })} /></label>}
                {selected.type === "MouseCastSpell" && <>{numberField(t("mapping.castRadius"), "cast_radius", selected.cast_radius)}{numberField(t("mapping.dragRadius"), "drag_radius", selected.drag_radius)}<label className="mapping-wide-field"><span>{t("mapping.castCenter")}</span><PositionInput value={selected.center} onChange={(center) => patch({ center })} /></label></>}
                {(selected.type === "Observation" || selected.type === "Fps" || selected.type === "Fire") && <>{numberField(t("mapping.sensitivityX"), "sensitivity_x", selected.sensitivity_x, 0, 0.1)}{numberField(t("mapping.sensitivityY"), "sensitivity_y", selected.sensitivity_y, 0, 0.1)}</>}
              </FieldSection>}

              {(selected.type === "MultipleTap" || selected.type === "Swipe") && (
                <FieldSection title={t("mapping.sequence") }>
                  <div className="mapping-sequence-list mapping-wide-field">
                    {sequenceItems.map((item, index) => {
                      const position = item.position;
                      return (
                        <div className={`mapping-sequence-row ${selected.type === "MultipleTap" ? "is-multiple" : "is-swipe"}`} key={index}>
                          <Tag>{index + 1}</Tag>
                          <PositionInput value={position} onChange={(next) => {
                            if (selected.type === "MultipleTap") patch({ items: selected.items.map((candidate, itemIndex) => itemIndex === index ? { ...candidate, position: next } : candidate) });
                            else patch({ positions: selected.positions.map((candidate, itemIndex) => itemIndex === index ? next : candidate) });
                          }} />
                          {selected.type === "MultipleTap" && <>
                            <Tooltip title={t("mapping.duration")}><InputNumber aria-label={t("mapping.duration")} prefix="D" min={0} value={item.duration} onChange={(duration) => duration !== null && patch({ items: selected.items.map((candidate, itemIndex) => itemIndex === index ? { ...candidate, duration } : candidate) })} /></Tooltip>
                            <Tooltip title={t("mapping.wait")}><InputNumber aria-label={t("mapping.wait")} prefix="W" min={0} value={item.wait} onChange={(wait) => wait !== null && patch({ items: selected.items.map((candidate, itemIndex) => itemIndex === index ? { ...candidate, wait } : candidate) })} /></Tooltip>
                          </>}
                          <Button
                            danger
                            size="small"
                            aria-label={t("mapping.deletePoint", { index: index + 1 })}
                            icon={<DeleteOutlined />}
                            disabled={selected.type === "Swipe" ? selected.positions.length <= 2 : selected.items.length <= 1}
                            onClick={() => selected.type === "MultipleTap" ? patch({ items: selected.items.filter((_, itemIndex) => itemIndex !== index) }) : patch({ positions: selected.positions.filter((_, itemIndex) => itemIndex !== index) })}
                          />
                        </div>
                      );
                    })}
                    <Button icon={<PlusOutlined />} onClick={() => {
                      if (selected.type === "MultipleTap") {
                        const last = selected.items.at(-1) ?? { position: selected.position, duration: 50, wait: 0 };
                        patch({ items: [...selected.items, { ...last, position: { ...last.position } }] });
                      } else {
                        const last = selected.positions.at(-1) ?? selected.position;
                        patch({ positions: [...selected.positions, { ...last }] });
                      }
                    }}>{t("mapping.addPoint")}</Button>
                  </div>
                </FieldSection>
              )}

              {selected.type === "Script" && (
                <FieldSection title={t("mapping.script") }>
                  <label className="mapping-wide-field"><span>{t("mapping.pressedScript")}</span><Input.TextArea rows={3} value={selected.pressed_script} onChange={(event) => patch({ pressed_script: event.target.value })} /></label>
                  <label className="mapping-wide-field"><span>{t("mapping.heldScript")}</span><Input.TextArea rows={3} value={selected.held_script} onChange={(event) => patch({ held_script: event.target.value })} /></label>
                  <label className="mapping-wide-field"><span>{t("mapping.releasedScript")}</span><Input.TextArea rows={3} value={selected.released_script} onChange={(event) => patch({ released_script: event.target.value })} /></label>
                  {numberField(t("mapping.interval"), "interval", selected.interval, 1)}
                </FieldSection>
              )}
            </div>
          ) : (
            <Empty image={Empty.PRESENTED_IMAGE_SIMPLE} description={t("mapping.noSelection")} />
          )}
        </div>
      )}
    </aside>
  );
}
