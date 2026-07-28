import type { ReactNode } from "react";

type Props = {
  status: ReactNode;
  functionControls: ReactNode;
  hardwareControls: ReactNode;
};

export function DeviceWindowToolbar({
  status,
  functionControls,
  hardwareControls,
}: Props) {
  return (
    <div className="stage-toolbar">
      {status}
      <div className="stage-toolbar-groups">
        <div className="stage-toolbar-group">{functionControls}</div>
        <div className="stage-toolbar-group">{hardwareControls}</div>
      </div>
    </div>
  );
}
