import HolderOutlined from "@ant-design/icons/es/icons/HolderOutlined";
import { Button, Tooltip } from "antd";
import { useState, type DragEvent, type ReactNode } from "react";
import { useTranslation } from "react-i18next";
import { swapWindowToolbarGroups, type WindowToolbarGroup } from "../deviceViewPreferences";

type Props = {
  status: ReactNode;
  functionControls: ReactNode;
  hardwareControls: ReactNode;
  order: WindowToolbarGroup[];
  onOrderChange: (order: WindowToolbarGroup[]) => void;
};

export function DeviceWindowToolbar({
  status,
  functionControls,
  hardwareControls,
  order,
  onOrderChange,
}: Props) {
  const { t } = useTranslation();
  const [dragging, setDragging] = useState<WindowToolbarGroup | null>(null);
  const [dropTarget, setDropTarget] = useState<WindowToolbarGroup | null>(null);
  const controls = { function: functionControls, hardware: hardwareControls };

  const startDrag = (kind: WindowToolbarGroup, event: DragEvent<HTMLElement>) => {
    event.dataTransfer.effectAllowed = "move";
    event.dataTransfer.setData("text/plain", kind);
    setDragging(kind);
  };
  const finishDrag = () => {
    setDragging(null);
    setDropTarget(null);
  };
  const drop = (target: WindowToolbarGroup, event: DragEvent<HTMLDivElement>) => {
    event.preventDefault();
    if (dragging && dragging !== target) {
      onOrderChange(swapWindowToolbarGroups(order, dragging, target));
    }
    finishDrag();
  };

  return (
    <div className="stage-toolbar">
      {status}
      <div className="stage-toolbar-groups">
        {order.map((kind) => {
          const label = t(kind === "hardware" ? "device.moveHardwareToolbar" : "device.moveFunctionToolbar");
          return (
            <div
              key={kind}
              className={`stage-toolbar-group${dragging === kind ? " is-dragging" : ""}${dropTarget === kind && dragging !== kind ? " is-drop-target" : ""}`}
              data-toolbar-group={kind}
              onDragEnter={() => setDropTarget(kind)}
              onDragOver={(event) => {
                event.preventDefault();
                event.dataTransfer.dropEffect = "move";
              }}
              onDrop={(event) => drop(kind, event)}
            >
              <Tooltip title={label}>
                <Button
                  className="stage-toolbar-drag-handle"
                  aria-label={label}
                  aria-grabbed={dragging === kind}
                  draggable
                  icon={<HolderOutlined />}
                  onDragStart={(event) => startDrag(kind, event)}
                  onDragEnd={finishDrag}
                />
              </Tooltip>
              {controls[kind]}
            </div>
          );
        })}
      </div>
    </div>
  );
}
