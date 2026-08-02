import CopyOutlined from "@ant-design/icons/es/icons/CopyOutlined";
import DeleteOutlined from "@ant-design/icons/es/icons/DeleteOutlined";
import InfoCircleOutlined from "@ant-design/icons/es/icons/InfoCircleOutlined";
import PlusOutlined from "@ant-design/icons/es/icons/PlusOutlined";
import SearchOutlined from "@ant-design/icons/es/icons/SearchOutlined";
import { Button, Dropdown, Empty, Input, InputNumber, Modal, Segmented, Select, Space, Switch, Tag, Tooltip, Typography } from "antd";
import { useEffect, useMemo, useState } from "react";
import { useTranslation } from "react-i18next";
import { convertEditorMappingType } from "../mappingEditor";
import { gamepadAxisNames, pointerButtonCode, readGamepadButtonPress, scrollBindingCode } from "../control";
import {
  hardwareButtons,
  keyboardBindingLabel,
  mappingBindingLabel,
  mappingContactIds,
  mappingLabel,
  mappingPosition,
  keyMappingTypes,
  updateMappingPosition,
  type ButtonBinding,
  type DirectionBinding,
  type HardwareBindings,
  type HardwareButtonName,
  type Mapping,
  type Position,
  type KeyMappingType,
} from "../types";

type Props = {
  mappings: Mapping[];
  selectedId: string | null;
  onSelect: (id: string) => void;
  onChange: (mapping: Mapping, mergeKey?: string) => void;
  onAdd: (type: KeyMappingType) => void;
  onDuplicate: (id: string) => void;
  onDelete: (id: string) => void;
  frameSize: { width: number; height: number };
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
  const [capturing, setCapturing] = useState(false);
  useEffect(() => {
    if (!capturing) return;
    let animationFrame = 0;
    const poll = () => {
      const code = readGamepadButtonPress();
      if (code) {
        setCapturing(false);
        onChange([code]);
        return;
      }
      animationFrame = window.requestAnimationFrame(poll);
    };
    animationFrame = window.requestAnimationFrame(poll);
    return () => window.cancelAnimationFrame(animationFrame);
  }, [capturing, onChange]);
  const stopCapture = () => setCapturing(false);
  return (
    <Input
      value={value.map(keyboardBindingLabel).join(" + ")}
      readOnly
      onFocus={() => setCapturing(true)}
      onBlur={stopCapture}
      onKeyDown={(event) => {
        stopCapture();
        event.preventDefault();
        event.stopPropagation();
        onChange(event.code === "Backspace" || event.code === "Delete" ? [] : modifierBinding(event));
      }}
      onPointerDown={(event) => {
        stopCapture();
        if (event.button === 0) return;
        const code = pointerButtonCode(event.button);
        if (!code) return;
        event.preventDefault();
        event.stopPropagation();
        onChange([code]);
      }}
      onWheel={(event) => {
        stopCapture();
        const code = scrollBindingCode(event.deltaY);
        if (!code) return;
        event.preventDefault();
        event.stopPropagation();
        onChange([code]);
      }}
      onContextMenu={(event) => event.preventDefault()}
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

function InfoHint({ hintKey }: { hintKey: string }) {
  const { t } = useTranslation();
  const hint = t(`mapping.parameterHints.${hintKey}`);
  return (
    <Tooltip title={hint}>
      <span className="mapping-info-icon" style={{ marginInlineStart: 4, cursor: "help" }} role="img" aria-label={hint}>
        <InfoCircleOutlined />
      </span>
    </Tooltip>
  );
}

function ParameterLabel({ label, hintKey }: { label: string; hintKey?: string }) {
  return (
    <span className="mapping-field-label">
      <span>{label}</span>
      {hintKey && <InfoHint hintKey={hintKey} />}
    </span>
  );
}

function DirectionInputs({ value, onChange, allowMode = true }: { value: DirectionBinding; onChange: (value: DirectionBinding) => void; allowMode?: boolean }) {
  const { t } = useTranslation();
  const axisOptions = [...new Set([
    ...gamepadAxisNames,
    ...(value.type === "JoyStick" ? [value.x, value.y] : []),
  ])].map((axis) => ({ value: axis, label: axis }));
  const mode = !allowMode ? null : (
    <label className="mapping-wide-field">
      <ParameterLabel label={t("mapping.type")} hintKey="direction" />
      <Select
        value={value.type}
        options={[{ value: "Button", label: t("mapping.buttonMode") }, { value: "JoyStick", label: t("mapping.joystickMode") }]}
        onChange={(type) => onChange(type === "Button"
          ? { type, up: [], down: [], left: [], right: [] }
          : { type, x: value.type === "JoyStick" ? value.x : "LeftStickX", y: value.type === "JoyStick" ? value.y : "LeftStickY" })}
      />
    </label>
  );
  if (value.type !== "Button") {
    return <>
      {mode}
      <div className="mapping-direction-grid mapping-wide-field">
        <label><ParameterLabel label={t("mapping.axisX")} hintKey="direction" /><Select value={value.x} options={axisOptions} onChange={(x) => onChange({ ...value, x })} /></label>
        <label><ParameterLabel label={t("mapping.axisY")} hintKey="direction" /><Select value={value.y} options={axisOptions} onChange={(y) => onChange({ ...value, y })} /></label>
      </div>
    </>;
  }
  return (
    <>
      {mode}
      <div className="mapping-direction-grid mapping-wide-field">
        {["up", "left", "down", "right"].map((direction) => (
          <label key={direction}>
            <ParameterLabel label={t(`mapping.directions.${direction}`)} hintKey="direction" />
            <BindingInput value={value[direction as "up" | "left" | "down" | "right"]} onChange={(binding) => onChange({ ...value, [direction]: binding })} />
          </label>
        ))}
      </div>
    </>
  );
}

function FieldSection({ title, hintKey, headerControl, children }: React.PropsWithChildren<{ title: string; hintKey?: string; headerControl?: React.ReactNode }>) {
  return (
    <section className="mapping-field-section">
      <Typography.Text
        strong
        style={headerControl ? { display: "flex", alignItems: "center", justifyContent: "space-between", gap: 8 } : undefined}
      >
        <span>{title}{hintKey && <InfoHint hintKey={hintKey} />}</span>
        {headerControl}
      </Typography.Text>
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
  frameSize,
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
  const patch = (values: object, coalesce = true) => selected && onChange(
    { ...selected, ...values } as Mapping,
    coalesce ? Object.keys(values).sort().join(",") : undefined,
  );
  const pointerId = selected && ("contactId" in selected ? selected.contactId : "pointer_id" in selected ? selected.pointer_id : null);
  const binding = selected && (selected.type === "touch" ? [selected.key] : "bind" in selected && Array.isArray(selected.bind) ? selected.bind : null);
  const primaryPosition = selected ? mappingPosition(selected) : null;
  const setPrimaryPosition = (position: Position) => selected && onChange(updateMappingPosition(selected, position), "position");
  const sequenceItems: { position: Position; duration?: number; wait?: number }[] = selected?.type === "MultipleTap"
    ? selected.items.map((item) => ({ position: item.position, duration: item.duration, wait: item.wait }))
    : selected?.type === "Swipe"
      ? selected.positions.map((position) => ({ position }))
      : [];
  const hasBehaviorFields = selected !== null && [
    "dpad", "DirectionPad", "PadCastSpell", "SingleTap", "RepeatTap", "Swipe",
    "MouseCastSpell", "Observation", "Fps", "Fire",
  ].includes(selected.type);
  const hasRandomizationFields = selected !== null && (
    "random_offset_x" in selected
    || "enable_randomization" in selected
    || "enable_initial_swipe_randomization" in selected
  );
  const numberField = (label: string, key: string, value: number, min = 0, step = 1, hintKey?: string) => (
    <label>
      <ParameterLabel label={label} hintKey={hintKey} />
      <InputNumber min={min} step={step} value={value} onChange={(next) => next !== null && patch({ [key]: next })} />
    </label>
  );
  const addMenu = {
    items: keyMappingTypes.map((type) => ({ key: type, label: t(`mapping.types.${type}`) })),
    onClick: ({ key }: { key: string }) => onAdd(key as KeyMappingType),
  };
  const selectedTypeLabel = selected
    ? t(`mapping.types.${selected.type === "touch" ? "SingleTap" : selected.type === "dpad" ? "DirectionPad" : selected.type}`)
    : "";
  const typeOptions = selected ? [
    ...(selected.type === "touch" || selected.type === "dpad"
      ? [{ value: selected.type, label: t("mapping.legacyType", { type: selectedTypeLabel }), disabled: true }]
      : []),
    ...keyMappingTypes.map((type) => ({ value: type, label: t(`mapping.types.${type}`) })),
  ] : [];
  const changeType = (type: KeyMappingType) => {
    if (!selected || selected.type === type) return;
    Modal.confirm({
      title: t("mapping.changeTypeTitle"),
      content: t("mapping.changeTypeDescription", {
        from: selectedTypeLabel,
        to: t(`mapping.types.${type}`),
      }),
      okText: t("mapping.changeTypeConfirm"),
      cancelText: t("common.cancel"),
      onOk: () => onChange(convertEditorMappingType(selected, type, frameSize)),
    });
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
                <label><ParameterLabel label={t("mapping.name")} hintKey="identity" /><Input value={mappingLabel(selected)} onChange={(event) => patch("label" in selected ? { label: event.target.value } : { note: event.target.value })} /></label>
                <label><ParameterLabel label={t("mapping.type")} hintKey="identity" /><Select value={selected.type} options={typeOptions} onChange={(type) => changeType(type as KeyMappingType)} /></label>
                {primaryPosition && <label className="mapping-wide-field"><ParameterLabel label={t("mapping.position")} hintKey="position" /><PositionInput value={primaryPosition} onChange={setPrimaryPosition} /></label>}
                {pointerId !== null && <label><ParameterLabel label={t("mapping.contactId")} hintKey="contact" /><InputNumber min={0} max={4} value={pointerId} onChange={(value) => {
                  if (value === null || (selected.type === "Fps" && selected.touch_mode.type === "dual" && value === selected.touch_mode.another_pointer_id)) return;
                  patch("contactId" in selected ? { contactId: value } : { pointer_id: value });
                }} /></label>}
                <label className="mapping-wide-field">
                  <ParameterLabel label={t("mapping.mappingId")} hintKey="mappingId" />
                  <Typography.Text copyable={{ text: selected.id }} style={{ overflowWrap: "anywhere" }}>{selected.id}</Typography.Text>
                </label>
              </FieldSection>

              {(binding || selected.type === "dpad" || selected.type === "DirectionPad" || selected.type === "PadCastSpell") && (
                <FieldSection title={t("mapping.input") }>
                  {binding && <label className="mapping-wide-field"><ParameterLabel label={t("mapping.keyboardBinding")} hintKey="binding" /><BindingInput value={binding} onChange={(value) => patch(selected.type === "touch" ? { key: value[0] ?? "" } : { bind: value })} /></label>}
                  {selected.type === "dpad" && <DirectionInputs allowMode={false} value={{ type: "Button", up: [selected.keys.up], down: [selected.keys.down], left: [selected.keys.left], right: [selected.keys.right] }} onChange={(value) => value.type === "Button" && patch({ keys: { up: value.up[0] ?? "", down: value.down[0] ?? "", left: value.left[0] ?? "", right: value.right[0] ?? "" } })} />}
                  {selected.type === "DirectionPad" && <DirectionInputs value={selected.bind} onChange={(bind) => patch({ bind })} />}
                  {selected.type === "PadCastSpell" && <DirectionInputs value={selected.pad_bind} onChange={(pad_bind) => patch({ pad_bind })} />}
                </FieldSection>
              )}

              {hasBehaviorFields && <FieldSection title={t("mapping.behavior") }>
                {selected.type === "dpad" && numberField(t("mapping.radius"), "radius", selected.radius, 0.01, 0.01, "range")}
                {selected.type === "DirectionPad" && <>
                  {numberField(t("mapping.offsetX"), "max_offset_x", selected.max_offset_x, 0, 1, "range")}
                  {numberField(t("mapping.offsetY"), "max_offset_y", selected.max_offset_y, 0, 1, "range")}
                  {numberField(t("mapping.initialDuration"), "initial_duration", selected.initial_duration, 0, 1, "duration")}
                  <label className="mapping-wide-field"><ParameterLabel label={t("mapping.upBoostKey")} hintKey="boost" /><Space.Compact block>
                    <Switch checked={selected.up_boost_key !== null} onChange={(enabled) => patch({ up_boost_key: enabled ? [] : null })} />
                    {selected.up_boost_key !== null && <BindingInput value={selected.up_boost_key} onChange={(up_boost_key) => patch({ up_boost_key })} />}
                  </Space.Compact></label>
                  {selected.up_boost_key !== null && numberField(t("mapping.upBoostScale"), "up_boost_scale", selected.up_boost_scale, 0.1, 0.1, "boost")}
                </>}
                {selected.type === "PadCastSpell" && <>
                  {numberField(t("mapping.dragRadius"), "drag_radius", selected.drag_radius, 0, 1, "range")}
                  <label><ParameterLabel label={t("mapping.blockDirectionPad")} hintKey="cast" /><Switch checked={selected.block_direction_pad} onChange={(block_direction_pad) => patch({ block_direction_pad })} /></label>
                </>}
                {selected.type === "SingleTap" && <>
                  <label><ParameterLabel label={t("mapping.sync")} hintKey="sync" /><Switch checked={selected.sync} onChange={(sync) => patch({ sync })} /></label>
                  {!selected.sync && numberField(t("mapping.duration"), "duration", selected.duration, 0, 1, "duration")}
                </>}
                {selected.type === "RepeatTap" && numberField(t("mapping.duration"), "duration", selected.duration, 0, 1, "duration")}
                {selected.type === "RepeatTap" && numberField(t("mapping.interval"), "interval", selected.interval, 1, 1, "timing")}
                {selected.type === "Swipe" && <>
                  {numberField(t("mapping.duration"), "duration", selected.duration, 0, 1, "duration")}
                </>}
                {(selected.type === "MouseCastSpell" || selected.type === "PadCastSpell") && <label><ParameterLabel label={t("mapping.releaseMode")} hintKey="release" /><Select value={selected.release_mode} options={(selected.type === "MouseCastSpell" ? ["OnPress", "OnRelease", "OnSecondPress"] : ["OnRelease", "OnSecondPress"]).map((value) => ({ value }))} onChange={(release_mode) => patch({ release_mode })} /></label>}
                {selected.type === "MouseCastSpell" && <>
                  {numberField(t("mapping.castRadius"), "cast_radius", selected.cast_radius, 0, 1, "cast")}
                  {numberField(t("mapping.dragRadius"), "drag_radius", selected.drag_radius, 0, 1, "range")}
                  {numberField(t("mapping.initialDuration"), "initial_duration", selected.initial_duration, 0, 1, "duration")}
                  <label><ParameterLabel label={t("mapping.castNoDirection")} hintKey="cast" /><Switch checked={selected.cast_no_direction} onChange={(cast_no_direction) => patch({ cast_no_direction })} /></label>
                  {numberField(t("mapping.horizontalScaleFactor"), "horizontal_scale_factor", selected.horizontal_scale_factor, 0.1, 0.1, "range")}
                  {numberField(t("mapping.verticalScaleFactor"), "vertical_scale_factor", selected.vertical_scale_factor, 0.1, 0.1, "range")}
                  <label className="mapping-wide-field"><ParameterLabel label={t("mapping.castCenter")} hintKey="cast" /><PositionInput value={selected.center} onChange={(center) => patch({ center })} /></label>
                </>}
                {selected.type === "Fire" && <label><ParameterLabel label={t("mapping.preserveFpsControl")} hintKey="fps" /><Switch checked={selected.preserve_fps_control} onChange={(preserve_fps_control) => patch({ preserve_fps_control })} /></label>}
                {(selected.type === "Observation" || selected.type === "Fps" || (selected.type === "Fire" && !selected.preserve_fps_control)) && <>{numberField(t("mapping.sensitivityX"), "sensitivity_x", selected.sensitivity_x, 0, 0.1, "range")}{numberField(t("mapping.sensitivityY"), "sensitivity_y", selected.sensitivity_y, 0, 0.1, "range")}</>}
                {selected.type === "Observation" && numberField(t("mapping.maxRadius"), "max_radius", selected.max_radius, 0, 1, "range")}
                {selected.type === "Fps" && <>
                  {numberField(t("mapping.offsetX"), "max_offset_x", selected.max_offset_x, 0, 1, "range")}
                  {numberField(t("mapping.offsetY"), "max_offset_y", selected.max_offset_y, 0, 1, "range")}
                  <label className="mapping-wide-field"><ParameterLabel label={t("mapping.fpsTouchMode")} hintKey="fps" /><Select
                    value={selected.touch_mode.type === "single" ? "single" : `dual-${selected.touch_mode.strategy}`}
                    options={[
                      { value: "single", label: t("mapping.fpsTouchModes.single") },
                      { value: "dual-delay", label: t("mapping.fpsTouchModes.dualDelay") },
                      { value: "dual-overlap", label: t("mapping.fpsTouchModes.dualOverlap") },
                    ]}
                    onChange={(mode) => {
                      const another_pointer_id = selected.touch_mode.type === "dual"
                        ? selected.touch_mode.another_pointer_id
                        : [0, 1, 2, 3, 4].find((identity) => identity !== selected.pointer_id) ?? 0;
                      const interval = "interval" in selected.touch_mode ? selected.touch_mode.interval : 0;
                      const touch_mode = mode === "single"
                        ? { type: "single" as const, interval }
                        : mode === "dual-delay"
                          ? { type: "dual" as const, another_pointer_id, strategy: "delay" as const, interval }
                          : { type: "dual" as const, another_pointer_id, strategy: "overlap" as const };
                      patch({ touch_mode }, false);
                    }}
                  /></label>
                  {selected.touch_mode.type === "dual" && <label><ParameterLabel label={t("mapping.secondaryContactId")} hintKey="fps" /><InputNumber min={0} max={4} value={selected.touch_mode.another_pointer_id} onChange={(another_pointer_id) => {
                    if (another_pointer_id === null || another_pointer_id === selected.pointer_id) return;
                    patch({ touch_mode: { ...selected.touch_mode, another_pointer_id } });
                  }} /></label>}
                  {(selected.touch_mode.type === "single" || selected.touch_mode.strategy === "delay") && <label><ParameterLabel label={t("mapping.recenterInterval")} hintKey="fps" /><InputNumber min={0} value={selected.touch_mode.interval} onChange={(interval) => interval !== null && patch({ touch_mode: { ...selected.touch_mode, interval } })} /></label>}
                </>}
              </FieldSection>}

              {hasRandomizationFields && selected && (
                <FieldSection
                  title={t("mapping.randomization")}
                  hintKey="random"
                  headerControl={"enable_randomization" in selected ? <Switch
                    aria-label={t("mapping.enableRandomization")}
                    checked={selected.enable_randomization}
                    onChange={(enable_randomization) => patch({ enable_randomization })}
                  /> : undefined}
                >
                  {selected.type === "DirectionPad" && selected.enable_randomization && <>
                    {numberField(t("mapping.randomOffsetX"), "random_offset_x", selected.random_offset_x, 0, 1, "random")}
                    {numberField(t("mapping.randomOffsetY"), "random_offset_y", selected.random_offset_y, 0, 1, "random")}
                    {numberField(t("mapping.randomDistanceMin"), "random_distance_min_scale", selected.random_distance_min_scale, 0, 0.05, "random")}
                    {numberField(t("mapping.randomDistanceMax"), "random_distance_max_scale", selected.random_distance_max_scale, 0, 0.05, "random")}
                    {numberField(t("mapping.jitterOffsetX"), "jitter_offset_x", selected.jitter_offset_x, 0, 1, "random")}
                    {numberField(t("mapping.jitterOffsetY"), "jitter_offset_y", selected.jitter_offset_y, 0, 1, "random")}
                  </>}
                  {selected.type === "MouseCastSpell" && <label><ParameterLabel label={t("mapping.initialSwipeRandomization")} hintKey="random" /><Switch checked={selected.enable_initial_swipe_randomization} onChange={(enable_initial_swipe_randomization) => patch({ enable_initial_swipe_randomization })} /></label>}
                  {"random_offset_x" in selected && selected.type !== "DirectionPad" && <>
                    {numberField(t("mapping.randomOffsetX"), "random_offset_x", selected.random_offset_x, 0, 1, "random")}
                    {numberField(t("mapping.randomOffsetY"), "random_offset_y", selected.random_offset_y, 0, 1, "random")}
                  </>}
                </FieldSection>
              )}

              {(selected.type === "MultipleTap" || selected.type === "Swipe") && (
                <FieldSection title={t("mapping.sequence") } hintKey="sequence">
                  <div className="mapping-sequence-list mapping-wide-field">
                    {sequenceItems.map((item, index) => {
                      const position = item.position;
                      return (
                        <div className={`mapping-sequence-row ${selected.type === "MultipleTap" ? "is-multiple" : "is-swipe"}`} key={index}>
                          <Tag>{index + 1}</Tag>
                          <PositionInput value={position} onChange={(next) => {
                            if (index === 0) {
                              onChange(updateMappingPosition(selected, next), "sequence-position");
                            } else if (selected.type === "MultipleTap") {
                              patch({ items: selected.items.map((candidate, itemIndex) => itemIndex === index ? { ...candidate, position: next } : candidate) });
                            } else {
                              patch({ positions: selected.positions.map((candidate, itemIndex) => itemIndex === index ? next : candidate) });
                            }
                          }} />
                          {selected.type === "MultipleTap" && <>
                            <InputNumber aria-label={t("mapping.duration")} prefix="D" suffix={<InfoHint hintKey="duration" />} min={0} value={item.duration} onChange={(duration) => duration !== null && patch({ items: selected.items.map((candidate, itemIndex) => itemIndex === index ? { ...candidate, duration } : candidate) })} />
                            <InputNumber aria-label={t("mapping.wait")} prefix="W" suffix={<InfoHint hintKey="timing" />} min={0} value={item.wait} onChange={(wait) => wait !== null && patch({ items: selected.items.map((candidate, itemIndex) => itemIndex === index ? { ...candidate, wait } : candidate) })} />
                          </>}
                          <Button
                            danger
                            size="small"
                            aria-label={t("mapping.deletePoint", { index: index + 1 })}
                            icon={<DeleteOutlined />}
                            disabled={selected.type === "Swipe" ? selected.positions.length <= 2 : selected.items.length <= 1}
                            onClick={() => {
                              if (selected.type === "MultipleTap") {
                                const items = selected.items.filter((_, itemIndex) => itemIndex !== index);
                                patch(index === 0 && items[0] ? { items, position: items[0].position } : { items }, false);
                              } else {
                                const positions = selected.positions.filter((_, itemIndex) => itemIndex !== index);
                                patch(index === 0 && positions[0] ? { positions, position: positions[0] } : { positions }, false);
                              }
                            }}
                          />
                        </div>
                      );
                    })}
                    <Button icon={<PlusOutlined />} onClick={() => {
                      if (selected.type === "MultipleTap") {
                        const last = selected.items.at(-1) ?? { position: selected.position, duration: 50, wait: 0 };
                        patch({ items: [...selected.items, { ...last, position: { ...last.position } }] }, false);
                      } else {
                        const last = selected.positions.at(-1) ?? selected.position;
                        patch({ positions: [...selected.positions, { ...last }] }, false);
                      }
                    }}>{t("mapping.addPoint")}</Button>
                  </div>
                </FieldSection>
              )}

              {selected.type === "Script" && (
                <FieldSection title={t("mapping.script") } hintKey="script">
                  <label className="mapping-wide-field"><ParameterLabel label={t("mapping.pressedScript")} hintKey="script" /><Input.TextArea rows={3} value={selected.pressed_script} onChange={(event) => patch({ pressed_script: event.target.value })} /></label>
                  <label className="mapping-wide-field"><ParameterLabel label={t("mapping.heldScript")} hintKey="script" /><Input.TextArea rows={3} value={selected.held_script} onChange={(event) => patch({ held_script: event.target.value })} /></label>
                  <label className="mapping-wide-field"><ParameterLabel label={t("mapping.releasedScript")} hintKey="script" /><Input.TextArea rows={3} value={selected.released_script} onChange={(event) => patch({ released_script: event.target.value })} /></label>
                  {numberField(t("mapping.interval"), "interval", selected.interval, 1, 1, "timing")}
                </FieldSection>
              )}

              {selected && "script_hooks" in selected && (
                <FieldSection title={t("mapping.scriptHooks") } hintKey="script">
                  <label className="mapping-wide-field"><ParameterLabel label={t("mapping.beforeScript")} hintKey="script" /><Input.TextArea rows={3} value={selected.script_hooks.before_script} onChange={(event) => patch({ script_hooks: { ...selected.script_hooks, before_script: event.target.value } })} /></label>
                  <label className="mapping-wide-field"><ParameterLabel label={t("mapping.afterScript")} hintKey="script" /><Input.TextArea rows={3} value={selected.script_hooks.after_script} onChange={(event) => patch({ script_hooks: { ...selected.script_hooks, after_script: event.target.value } })} /></label>
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
