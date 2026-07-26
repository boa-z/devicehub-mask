//! Desktop path binding for runtime-owned House Arrest application storage.

use std::path::PathBuf;

#[cfg(test)]
pub(crate) use devicehub_runtime::{
    APP_DOCUMENT_TRANSFER_CANCELLED as TRANSFER_CANCELLED, AppDocumentActivityState,
    AppDocumentKind, AppDocumentTransfer,
};
pub(crate) use devicehub_runtime::{
    AppDocumentActivitySlot, AppDocumentActivityView, AppDocumentEntry, AppDocumentList,
    AppStorageScope, AppStorageTransport,
    is_app_document_transfer_cancelled as is_transfer_cancelled, serve_app_documents as serve,
};

pub(crate) type AppDocumentCommand = devicehub_runtime::AppDocumentCommand<PathBuf>;
