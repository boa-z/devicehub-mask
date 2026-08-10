//! Authenticated device identity resolved once for a request.

use std::path::PathBuf;
use std::sync::Arc;

#[derive(Clone)]
pub(crate) struct DeviceScope {
    pub(crate) selection_id: Arc<str>,
    pub(crate) session: devicehub_runtime::DeviceSessionClient<PathBuf>,
}

impl DeviceScope {
    pub(crate) fn new(
        selection_id: impl Into<Arc<str>>,
        session: devicehub_runtime::DeviceSessionClient<PathBuf>,
    ) -> Self {
        Self {
            selection_id: selection_id.into(),
            session,
        }
    }
}
