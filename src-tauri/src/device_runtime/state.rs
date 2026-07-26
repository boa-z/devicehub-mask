//! Runtime state ports bound to desktop host path types.

pub(crate) use devicehub_runtime::ClipboardSlot;

pub(crate) use devicehub_core::{
    ActiveSlot, AppOperationSlot, DeviceListSlot, ErrorSlot, LocationStatusSlot, OrientationSlot,
    StatusSlot, VideoCounters,
};

pub(crate) type InputSink = devicehub_runtime::SessionCommandSlot<std::path::PathBuf>;
