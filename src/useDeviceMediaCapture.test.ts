import { describe, expect, it } from "vitest";
import { recordingFilename, screenshotFilename, selectRecordingMimeType } from "./useDeviceMediaCapture";

const now = new Date("2026-07-25T01:02:03.456Z");

describe("device media capture", () => {
  it("builds portable screenshot and recording filenames", () => {
    expect(screenshotFilename(' Boa/iPhone:*? ', 1290, 2796, now))
      .toBe("devicehub-mask_Boa-iPhone-_1290x2796_2026-07-25T01-02-03-456Z.png");
    expect(recordingFilename("   ", "webm", now))
      .toBe("devicehub-mask_iPhone_2026-07-25T01-02-03-456Z.webm");
  });

  it("selects the first recording format supported by the WebView", () => {
    expect(selectRecordingMimeType((mimeType) => mimeType === "video/webm;codecs=vp8"))
      .toBe("video/webm;codecs=vp8");
    expect(selectRecordingMimeType(() => false)).toBeUndefined();
  });
});
