import { describe, expect, it } from "vitest";
import { bindingForScope, resolveAppProfileBinding } from "./profileBindings";
import type { AppBindingConflict, AppProfileBinding } from "./types";

const bindings: AppProfileBinding[] = [
  { bundle_id: "com.example.game", profile: "iphone", target_resolution: { width: 1290, height: 2796 } },
  { bundle_id: "com.example.game", profile: "ipad", target_resolution: { width: 1620, height: 2160 } },
];

describe("resolution-aware app profile bindings", () => {
  it("selects only an exact device resolution", () => {
    expect(resolveAppProfileBinding("com.example.game", { width: 1290, height: 2796 }, bindings, []).binding?.profile).toBe("iphone");
    expect(resolveAppProfileBinding("com.example.game", { width: 1620, height: 2160 }, bindings, []).binding?.profile).toBe("ipad");
    expect(resolveAppProfileBinding("com.example.game", { width: 1170, height: 2532 }, bindings, []).binding).toBeUndefined();
  });

  it("keeps conflicts isolated to their resolution scope", () => {
    const conflicts: AppBindingConflict[] = [
      { bundle_id: "com.example.game", target_resolution: { width: 1290, height: 2796 } },
    ];
    expect(resolveAppProfileBinding("com.example.game", { width: 1290, height: 2796 }, bindings, conflicts).conflict).toBe(true);
    expect(resolveAppProfileBinding("com.example.game", { width: 1620, height: 2160 }, bindings, conflicts).binding?.profile).toBe("ipad");
    expect(bindingForScope("com.example.game", null, bindings)).toBeUndefined();
  });
});
