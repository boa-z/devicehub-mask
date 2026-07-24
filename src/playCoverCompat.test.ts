import { describe, expect, it } from "vitest";
import { keyboardCodeForUsage, keyboardUsage } from "./control";
import { importPlayCoverConfig, parsePlayCoverPlist } from "./playCoverCompat";

const transform = (xCoord: number, yCoord: number, size = 20) => ({ xCoord, yCoord, size });

describe("PlayCover compatibility", () => {
  it("converts USB HID usages back to browser keyboard codes", () => {
    expect(keyboardCodeForUsage(4)).toBe("KeyA");
    expect(keyboardCodeForUsage(44)).toBe("Space");
    expect(keyboardCodeForUsage(82)).toBe("ArrowUp");
    expect(keyboardCodeForUsage(0x46)).toBe("PrintScreen");
    expect(keyboardCodeForUsage(225)).toBe("ShiftLeft");
    expect(keyboardCodeForUsage(0x68)).toBe("F13");
    expect(keyboardUsage("F13")).toBe(0x68);
    expect(keyboardUsage("F24")).toBe(0x73);
    expect(keyboardCodeForUsage(-1)).toBeUndefined();
  });

  it("imports buttons, draggable buttons, joysticks, and the app binding", () => {
    const result = importPlayCoverConfig({
      version: "2.0.0",
      bundleIdentifier: "com.example.game",
      buttonModels: [{ keyCode: 44, keyName: "Jump", transform: transform(0.8, 0.7) }],
      draggableButtonModels: [{ keyCode: 20, keyName: "Aim", transform: transform(0.7, 0.8, 10) }],
      joystickModel: [{ upKeyCode: 26, downKeyCode: 22, leftKeyCode: 4, rightKeyCode: 7, transform: transform(0.2, 0.75, 24) }],
      mouseAreaModel: [],
    }, "game", { width: 1600, height: 900 });
    expect(result).toMatchObject({ imported: 3, skipped: 0 });
    expect(result.profile.bundleIdentifiers).toEqual(["com.example.game"]);
    expect(result.profile.mappings[0]).toMatchObject({ type: "SingleTap", bind: ["Space"], position: { x: 0.8, y: 0.7 } });
    expect(result.profile.mappings[1]).toMatchObject({ type: "MouseCastSpell", bind: ["KeyQ"], position: { x: 0.7, y: 0.8 } });
    expect(result.profile.mappings[2]).toMatchObject({
      type: "DirectionPad",
      bind: { type: "Button", up: ["KeyW"], down: ["KeyS"], left: ["KeyA"], right: ["KeyD"] },
      position: { x: 0.2, y: 0.75 },
    });
  });

  it("skips unsupported mouse and controller bindings without inventing keys", () => {
    const result = importPlayCoverConfig({
      version: "2.0.0",
      buttonModels: [{ keyCode: -1, keyName: "LMB", transform: transform(0.5, 0.5) }],
      draggableButtonModels: [],
      joystickModel: [],
      mouseAreaModel: [{ transform: transform(0.5, 0.5) }],
    }, "game", { width: 1600, height: 900 });
    expect(result).toMatchObject({ imported: 0, skipped: 2 });
  });

  it("parses an ordered XML plist and only accepts the standard Apple document type", async () => {
    const xml = `<?xml version="1.0" encoding="UTF-8"?>
      <plist version="1.0"><dict>
        <key>version</key><string>2.0.0</string>
        <key>bundleIdentifier</key><string>com.example.game</string>
        <key>buttonModels</key><array/>
        <key>draggableButtonModels</key><array/>
        <key>joystickModel</key><array/>
        <key>mouseAreaModel</key><array/>
      </dict></plist>`;
    await expect(parsePlayCoverPlist(xml)).resolves.toMatchObject({ version: "2.0.0", bundleIdentifier: "com.example.game" });
    await expect(parsePlayCoverPlist(xml.replace(
      "<plist",
      `<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd"><plist`,
    ))).resolves.toMatchObject({ version: "2.0.0" });
    await expect(parsePlayCoverPlist(`<!DOCTYPE plist><plist><dict/></plist>`)).rejects.toThrow();
    await expect(parsePlayCoverPlist(`<!ENTITY key "value"><plist><dict/></plist>`)).rejects.toThrow();
  });
});
