import { Modal, Spin, message } from "antd";
import { useEffect, useState } from "react";

type Request = (path: string, init?: RequestInit) => Promise<Response>;

type Props = {
  open: boolean;
  fileName: string;
  requestPath: string | null;
  request: Request;
  onClose: () => void;
};

export function AfcImagePreviewModal({ open, fileName, requestPath, request, onClose }: Props) {
  const [source, setSource] = useState<string | null>(null);

  useEffect(() => {
    if (!open || !requestPath) {
      setSource(null);
      return;
    }

    const controller = new AbortController();
    let objectUrl: string | null = null;
    setSource(null);

    void request(requestPath, { signal: controller.signal })
      .then(async (response) => {
        if (!response.ok) {
          throw new Error((await response.text()) || `${response.status} ${response.statusText}`);
        }
        return response.blob();
      })
      .then((blob) => {
        if (controller.signal.aborted) return;
        objectUrl = URL.createObjectURL(blob);
        setSource(objectUrl);
      })
      .catch((previewError: unknown) => {
        if (controller.signal.aborted) return;
        void message.error(String(previewError));
      });

    return () => {
      controller.abort();
      if (objectUrl) URL.revokeObjectURL(objectUrl);
    };
  }, [open, request, requestPath]);

  return (
    <Modal
      className="afc-image-preview-modal"
      open={open}
      title={fileName}
      footer={null}
      width={860}
      destroyOnHidden
      onCancel={onClose}
    >
      <div className="afc-image-preview" aria-busy={open && !source}>
        {open && !source && <Spin />}
        {source && <img src={source} alt={fileName} />}
      </div>
    </Modal>
  );
}
