import { message } from "antd";
import { useCallback, useEffect, useRef, useState, type RefObject } from "react";
import type { TFunction } from "i18next";
import { parsePngDimensions } from "../../deviceScreenshot";
import { logFrontend } from "../../diagnostics";
import { showErrorMessage } from "../../errorMessage";
import type { BackendRequest } from "../../shared/backend/client";

const recordingMimeTypes = [
  "video/mp4;codecs=avc1.42E01E",
  "video/mp4",
  "video/webm;codecs=vp9",
  "video/webm;codecs=vp8",
  "video/webm",
] as const;

export type CapturedScreenshot = {
  blob: Blob;
  url: string;
  width: number;
  height: number;
};

type Options = {
  canvasRef: RefObject<HTMLCanvasElement | null>;
  canvasReadyRef: RefObject<boolean>;
  hasFrame: boolean;
  activeDeviceId: string | null;
  activeDeviceName: string;
  devicePageActive: boolean;
  request: BackendRequest;
  onUseCapturedBackground: () => void;
  t: TFunction;
};

function safeDeviceName(deviceName: string) {
  return deviceName.trim().replace(/[<>:"/\\|?*]+/g, "-") || "iPhone";
}

function filenameTimestamp(now: Date) {
  return now.toISOString().replace(/[:.]/g, "-");
}

export function screenshotFilename(deviceName: string, width: number, height: number, now = new Date()) {
  return `devicehub-mask_${safeDeviceName(deviceName)}_${width}x${height}_${filenameTimestamp(now)}.png`;
}

export function recordingFilename(deviceName: string, extension: string, now = new Date()) {
  return `devicehub-mask_${safeDeviceName(deviceName)}_${filenameTimestamp(now)}.${extension}`;
}

export function selectRecordingMimeType(isSupported: (mimeType: string) => boolean) {
  return recordingMimeTypes.find(isSupported);
}

function canvasPng(canvas: HTMLCanvasElement) {
  return new Promise<Blob | null>((resolve) => canvas.toBlob(resolve, "image/png"));
}

function downloadUrl(url: string, filename: string) {
  const link = document.createElement("a");
  link.href = url;
  link.download = filename;
  document.body.appendChild(link);
  link.click();
  link.remove();
}

/** Owns browser capture resources independently from the device workspace. */
export function useDeviceMediaCapture({
  canvasRef,
  canvasReadyRef,
  hasFrame,
  activeDeviceId,
  activeDeviceName,
  devicePageActive,
  request,
  onUseCapturedBackground,
  t,
}: Options) {
  const [capturedScreenshot, setCapturedScreenshot] = useState<CapturedScreenshot | null>(null);
  const [screenshotBusy, setScreenshotBusy] = useState(false);
  const [recording, setRecording] = useState(false);
  const capturedScreenshotRef = useRef<CapturedScreenshot | null>(null);
  const screenshotInFlightRef = useRef(false);
  const recorderRef = useRef<MediaRecorder | null>(null);
  const recordingStreamRef = useRef<MediaStream | null>(null);
  const recordingChunksRef = useRef<Blob[]>([]);
  const mountedRef = useRef(true);

  const captureMappingScreenshot = useCallback(async (selectBackground: boolean) => {
    const canvas = canvasRef.current;
    if (!canvas || !hasFrame) {
      void message.warning(t("mapping.screenshotUnavailable"));
      return null;
    }
    const blob = await canvasPng(canvas);
    if (!mountedRef.current) return null;
    if (!blob) {
      void showErrorMessage(t("mapping.screenshotFailed"));
      return null;
    }
    const next = {
      blob,
      url: URL.createObjectURL(blob),
      width: canvas.width,
      height: canvas.height,
    };
    const previous = capturedScreenshotRef.current;
    capturedScreenshotRef.current = next;
    setCapturedScreenshot(next);
    if (selectBackground) onUseCapturedBackground();
    if (previous) URL.revokeObjectURL(previous.url);
    void message.success(t("mapping.screenshotCaptured"));
    return next;
  }, [canvasRef, hasFrame, onUseCapturedBackground, t]);

  const saveMappingScreenshot = useCallback(async (useLiveFrame: boolean) => {
    const screenshot = useLiveFrame
      ? await captureMappingScreenshot(false)
      : capturedScreenshotRef.current ?? await captureMappingScreenshot(false);
    if (!screenshot) return;
    downloadUrl(
      screenshot.url,
      screenshotFilename(activeDeviceName, screenshot.width, screenshot.height),
    );
    void message.success(t("mapping.screenshotSaved"));
  }, [activeDeviceName, captureMappingScreenshot, t]);

  const saveDeviceScreenshot = useCallback(async () => {
    if (screenshotInFlightRef.current) return;
    screenshotInFlightRef.current = true;
    setScreenshotBusy(true);
    try {
      let blob: Blob | null = null;
      let dimensions: { width: number; height: number } | null = null;
      if (activeDeviceId) {
        try {
          const response = await request("/api/device/screenshot");
          if (!response.ok) throw new Error((await response.text()) || response.statusText);
          const native = await response.blob();
          const header = new Uint8Array(await native.slice(0, 24).arrayBuffer());
          dimensions = parsePngDimensions(header);
          if (!dimensions) throw new Error("device returned an invalid PNG screenshot");
          blob = native;
        } catch (error) {
          logFrontend("warn", "screenshot", "native_capture", error);
        }
      }
      if (!blob || !dimensions) {
        const canvas = canvasRef.current;
        if (!canvas || !canvasReadyRef.current) {
          void message.warning(t("device.screenshotUnavailable"));
          return;
        }
        blob = await canvasPng(canvas);
        dimensions = { width: canvas.width, height: canvas.height };
      }
      if (!blob) {
        void showErrorMessage(t("device.screenshotFailed"));
        return;
      }
      if (!mountedRef.current) return;
      const url = URL.createObjectURL(blob);
      downloadUrl(url, screenshotFilename(activeDeviceName, dimensions.width, dimensions.height));
      window.setTimeout(() => URL.revokeObjectURL(url), 1_000);
      void message.success(t("device.screenshotSaved"));
    } finally {
      screenshotInFlightRef.current = false;
      if (mountedRef.current) setScreenshotBusy(false);
    }
  }, [activeDeviceId, activeDeviceName, canvasReadyRef, canvasRef, request, t]);

  const stopDeviceRecording = useCallback(() => {
    const recorder = recorderRef.current;
    if (recorder && recorder.state !== "inactive") recorder.stop();
  }, []);

  const toggleDeviceRecording = useCallback(() => {
    const activeRecorder = recorderRef.current;
    if (activeRecorder && activeRecorder.state !== "inactive") {
      activeRecorder.stop();
      return;
    }
    const canvas = canvasRef.current;
    if (!canvas || !canvasReadyRef.current || typeof MediaRecorder === "undefined" || typeof canvas.captureStream !== "function") {
      void message.warning(t("device.recordingUnavailable"));
      return;
    }
    try {
      const stream = canvas.captureStream(60);
      const mimeType = selectRecordingMimeType(MediaRecorder.isTypeSupported.bind(MediaRecorder));
      const recorder = mimeType ? new MediaRecorder(stream, { mimeType }) : new MediaRecorder(stream);
      recordingChunksRef.current = [];
      recordingStreamRef.current = stream;
      recorderRef.current = recorder;
      recorder.ondataavailable = (event) => {
        if (event.data.size > 0) recordingChunksRef.current.push(event.data);
      };
      recorder.onerror = (event) => {
        logFrontend("warn", "video", "recording", event.error);
        void showErrorMessage(t("device.recordingFailed", { error: event.error.message }));
      };
      recorder.onstop = () => {
        const chunks = recordingChunksRef.current;
        const recordedType = recorder.mimeType || mimeType || "video/webm";
        recordingStreamRef.current?.getTracks().forEach((track) => track.stop());
        recordingStreamRef.current = null;
        recorderRef.current = null;
        recordingChunksRef.current = [];
        if (!mountedRef.current) return;
        setRecording(false);
        if (chunks.length === 0) return;
        const blob = new Blob(chunks, { type: recordedType });
        const url = URL.createObjectURL(blob);
        downloadUrl(url, recordingFilename(activeDeviceName, recordedType.includes("mp4") ? "mp4" : "webm"));
        window.setTimeout(() => URL.revokeObjectURL(url), 1_000);
        void message.success(t("device.recordingSaved"));
      };
      recorder.start(1_000);
      setRecording(true);
    } catch (error) {
      recordingStreamRef.current?.getTracks().forEach((track) => track.stop());
      recordingStreamRef.current = null;
      recorderRef.current = null;
      setRecording(false);
      logFrontend("warn", "video", "start_recording", error);
      void showErrorMessage(t("device.recordingFailed", { error: String(error) }));
    }
  }, [activeDeviceName, canvasReadyRef, canvasRef, t]);

  useEffect(() => {
    if (!devicePageActive || !activeDeviceId) stopDeviceRecording();
  }, [activeDeviceId, devicePageActive, stopDeviceRecording]);

  useEffect(() => {
    mountedRef.current = true;
    return () => {
      mountedRef.current = false;
      if (capturedScreenshotRef.current) URL.revokeObjectURL(capturedScreenshotRef.current.url);
      const recorder = recorderRef.current;
      if (recorder && recorder.state !== "inactive") recorder.stop();
      recordingStreamRef.current?.getTracks().forEach((track) => track.stop());
    };
  }, []);

  const recordingSupported = typeof MediaRecorder !== "undefined"
    && typeof HTMLCanvasElement !== "undefined"
    && typeof HTMLCanvasElement.prototype.captureStream === "function";

  return {
    capturedScreenshot,
    screenshotBusy,
    recording,
    recordingSupported,
    captureMappingScreenshot,
    saveMappingScreenshot,
    saveDeviceScreenshot,
    stopDeviceRecording,
    toggleDeviceRecording,
  };
}
