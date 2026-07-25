// The package ships per-icon declarations only under the equivalent `lib` path.
declare module "@ant-design/icons/es/icons/*" {
  import type { ForwardRefExoticComponent, RefAttributes } from "react";
  import type { AntdIconProps } from "@ant-design/icons/lib/components/AntdIcon";

  const icon: ForwardRefExoticComponent<
    Omit<AntdIconProps, "ref"> & RefAttributes<HTMLSpanElement>
  >;
  export default icon;
}
