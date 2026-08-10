//! Cross-session ownership helpers for the outer runtime manager.

use std::collections::{HashMap, HashSet};

use tokio::sync::mpsc::UnboundedReceiver;

use super::{ManagedSessionViews, SWITCH_GRACE};
use crate::{DeviceSessionCommand, SessionEndpoint};

pub(super) fn running_selection_for_udid<'a>(
    running: &'a HashSet<String>,
    endpoints: &HashMap<String, SessionEndpoint>,
    udid: &str,
) -> Option<&'a str> {
    running.iter().find_map(|selection_id| {
        endpoints
            .get(selection_id)
            .filter(|endpoint| endpoint.udid() == udid)
            .map(|_| selection_id.as_str())
    })
}

pub(super) async fn stop_all_sessions<HostPath>(
    sessions: &HashMap<String, ManagedSessionViews<HostPath>>,
    running: &mut HashSet<String>,
    ended: &mut UnboundedReceiver<String>,
) {
    for selection_id in running.iter() {
        if let Some(session) = sessions.get(selection_id) {
            session.supervisor.stop();
            session.commands.send(DeviceSessionCommand::Shutdown);
        }
    }
    let deadline = tokio::time::sleep(SWITCH_GRACE);
    tokio::pin!(deadline);
    while !running.is_empty() {
        tokio::select! {
            ended_id = ended.recv() => {
                let Some(ended_id) = ended_id else { break };
                running.remove(&ended_id);
            }
            _ = &mut deadline => break,
        }
    }
}
