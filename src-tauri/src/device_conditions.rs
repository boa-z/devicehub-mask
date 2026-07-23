//! Supervised DVT network and thermal condition simulation.

use std::collections::HashSet;
use std::sync::{Arc, Mutex};
use std::time::Duration;

#[cfg(test)]
use idevice::dvt::condition_inducer::ConditionProfile;
use idevice::dvt::condition_inducer::{ConditionInducerClient, ConditionInducerGroup};
use idevice::dvt::remote_server::RemoteServerClient;
use idevice::rsd::RsdHandshake;
use idevice::tcp::handle::AdapterHandle;
use idevice::{ReadWrite, RsdService};
use serde::Serialize;
use tokio::sync::{mpsc, oneshot, watch};

use crate::supervisor::{ServiceReporter, reconnect_backoff, wait_for_retry};

const CONNECT_TIMEOUT: Duration = Duration::from_secs(6);
const OPERATION_TIMEOUT: Duration = Duration::from_secs(6);
const SHUTDOWN_CLEAR_TIMEOUT: Duration = Duration::from_secs(2);
const MAX_GROUPS: usize = 32;
const MAX_PROFILES: usize = 256;
const MAX_IDENTIFIER_BYTES: usize = 256;
const MAX_DESCRIPTION_CHARS: usize = 512;

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct DeviceConditionProfile {
    pub identifier: String,
    pub description: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct DeviceConditionGroup {
    pub identifier: String,
    pub profiles: Vec<DeviceConditionProfile>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ActiveDeviceCondition {
    pub group_identifier: String,
    pub profile_identifier: String,
    pub description: String,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
pub struct DeviceConditionStatus {
    pub available: bool,
    pub groups: Vec<DeviceConditionGroup>,
    pub active: Option<ActiveDeviceCondition>,
    pub cleanup_pending: bool,
    pub error: Option<String>,
}

#[derive(Clone, Default)]
pub struct DeviceConditionSlot(Arc<Mutex<DeviceConditionStatus>>);

impl DeviceConditionSlot {
    pub fn set(&self, status: DeviceConditionStatus) {
        *self.0.lock().unwrap() = status;
    }

    pub fn get(&self) -> DeviceConditionStatus {
        self.0.lock().unwrap().clone()
    }

    pub fn reset(&self) {
        self.set(DeviceConditionStatus::default());
    }
}

#[derive(Debug)]
pub enum DeviceConditionCommand {
    Apply {
        group_identifier: String,
        profile_identifier: String,
        expires_at: tokio::time::Instant,
        reply: oneshot::Sender<Result<(), String>>,
    },
    Clear {
        expires_at: tokio::time::Instant,
        reply: oneshot::Sender<Result<(), String>>,
    },
}

pub fn validate_identifiers(
    group_identifier: &str,
    profile_identifier: &str,
) -> Result<(), String> {
    validate_identifier(group_identifier, "condition group")?;
    validate_identifier(profile_identifier, "condition profile")
}

pub async fn supervise(
    adapter: AdapterHandle,
    handshake: RsdHandshake,
    mut commands: mpsc::Receiver<DeviceConditionCommand>,
    status: DeviceConditionSlot,
    reporter: ServiceReporter,
    mut shutdown: watch::Receiver<bool>,
) {
    let previous = status.get();
    let mut current = previous.active;
    let mut cleanup_pending = previous.cleanup_pending || current.is_some();
    let mut attempt = 0;
    loop {
        if *shutdown.borrow() {
            break;
        }
        attempt += 1;
        reporter.connecting(attempt);
        let mut remote = match connect_remote(adapter.clone(), handshake.clone()).await {
            Ok(remote) => remote,
            Err(error) => {
                cleanup_pending |= current.is_some();
                mark_failed(&status, current.clone(), cleanup_pending, error.clone());
                reporter.retrying(attempt, error);
                if !wait_for_retry(&mut shutdown, reconnect_backoff(attempt - 1)).await {
                    break;
                }
                continue;
            }
        };
        let mut client =
            match tokio::time::timeout(CONNECT_TIMEOUT, ConditionInducerClient::new(&mut remote))
                .await
            {
                Ok(Ok(client)) => client,
                Ok(Err(error)) => {
                    let error = format!("DVT condition channel unavailable: {error:?}");
                    cleanup_pending |= current.is_some();
                    mark_failed(&status, current.clone(), cleanup_pending, error.clone());
                    reporter.retrying(attempt, error);
                    if !wait_for_retry(&mut shutdown, reconnect_backoff(attempt - 1)).await {
                        break;
                    }
                    continue;
                }
                Err(_) => {
                    let error = "DVT condition channel timed out".to_string();
                    cleanup_pending |= current.is_some();
                    mark_failed(&status, current.clone(), cleanup_pending, error.clone());
                    reporter.retrying(attempt, error);
                    if !wait_for_retry(&mut shutdown, reconnect_backoff(attempt - 1)).await {
                        break;
                    }
                    continue;
                }
            };

        // Establish a known baseline even after an app crash or a previous
        // session that could not record its cleanup state.
        if let Err(error) = timed_clear(&mut client).await {
            cleanup_pending = true;
            mark_failed(&status, current.clone(), true, error.clone());
            reporter.retrying(attempt, error);
            if !wait_for_retry(&mut shutdown, reconnect_backoff(attempt - 1)).await {
                break;
            }
            continue;
        }
        cleanup_pending = false;
        current = None;

        let groups = match timed_list(&mut client).await {
            Ok(groups) => normalize_groups(groups),
            Err(error) => {
                mark_failed(&status, current.clone(), cleanup_pending, error.clone());
                reporter.retrying(attempt, error);
                if !wait_for_retry(&mut shutdown, reconnect_backoff(attempt - 1)).await {
                    break;
                }
                continue;
            }
        };
        status.set(ready_status(&groups, current.clone()));
        reporter.ready(attempt);

        let failure = loop {
            tokio::select! {
                changed = shutdown.changed() => {
                    if changed.is_err() || *shutdown.borrow() {
                        break None;
                    }
                }
                command = commands.recv() => {
                    let Some(command) = command else { break None };
                    match command {
                        DeviceConditionCommand::Apply { group_identifier, profile_identifier, expires_at, reply } => {
                            if reply.is_closed() || tokio::time::Instant::now() >= expires_at {
                                let _ = reply.send(Err("device condition request expired".into()));
                                continue;
                            }
                            let selected = find_profile(&groups, &group_identifier, &profile_identifier);
                            let Some(selected) = selected else {
                                let _ = reply.send(Err("selected device condition is unavailable".into()));
                                continue;
                            };
                            if current.as_ref() == Some(&selected) {
                                let _ = reply.send(Ok(()));
                                continue;
                            }
                            if current.is_some()
                                && let Err(error) = timed_clear_until(&mut client, expires_at).await
                            {
                                cleanup_pending = true;
                                let _ = reply.send(Err(error.clone()));
                                break Some(error);
                            }
                            match timed_apply_until(&mut client, &selected, expires_at).await {
                                Ok(()) => {
                                    current = Some(selected);
                                    cleanup_pending = false;
                                    status.set(ready_status(&groups, current.clone()));
                                    tracing::info!(
                                        component = "device_conditions",
                                        operation = "apply",
                                        group_identifier,
                                        profile_identifier,
                                        "device condition applied"
                                    );
                                    let _ = reply.send(Ok(()));
                                }
                                Err(error) => {
                                    // The device may have applied the condition before the reply failed.
                                    current = Some(selected);
                                    cleanup_pending = true;
                                    let _ = reply.send(Err(error.clone()));
                                    break Some(error);
                                }
                            }
                        }
                        DeviceConditionCommand::Clear { expires_at, reply } => {
                            if reply.is_closed() || tokio::time::Instant::now() >= expires_at {
                                let _ = reply.send(Err("device condition request expired".into()));
                                continue;
                            }
                            match timed_clear_until(&mut client, expires_at).await {
                            Ok(()) => {
                                current = None;
                                cleanup_pending = false;
                                status.set(ready_status(&groups, None));
                                tracing::info!(
                                    component = "device_conditions",
                                    operation = "clear",
                                    "device condition cleared"
                                );
                                let _ = reply.send(Ok(()));
                            }
                            Err(error) => {
                                cleanup_pending = true;
                                let _ = reply.send(Err(error.clone()));
                                break Some(error);
                            }
                            }
                        }
                    }
                }
            }
        };

        if let Some(error) = failure {
            cleanup_pending |= current.is_some();
            mark_failed(&status, current.clone(), cleanup_pending, error.clone());
            reporter.retrying(attempt, error);
            if !wait_for_retry(&mut shutdown, reconnect_backoff(attempt - 1)).await {
                break;
            }
            continue;
        }
        if cleanup_pending || current.is_some() {
            let cleared =
                tokio::time::timeout(SHUTDOWN_CLEAR_TIMEOUT, client.disable_condition()).await;
            if matches!(cleared, Ok(Ok(()))) {
                current = None;
                cleanup_pending = false;
            } else {
                cleanup_pending = true;
                mark_failed(
                    &status,
                    current.clone(),
                    true,
                    "device condition shutdown cleanup was not confirmed".into(),
                );
            }
            tracing::info!(
                component = "device_conditions",
                operation = "shutdown_clear",
                success = matches!(cleared, Ok(Ok(()))),
                "device condition shutdown cleanup finished"
            );
        }
        break;
    }
    if !cleanup_pending && current.is_none() {
        status.reset();
    }
    reporter.stopped(attempt);
}

async fn connect_remote(
    mut adapter: AdapterHandle,
    mut handshake: RsdHandshake,
) -> Result<RemoteServerClient<Box<dyn ReadWrite + 'static>>, String> {
    tokio::time::timeout(CONNECT_TIMEOUT, async {
        let mut remote = RemoteServerClient::connect_rsd(&mut adapter, &mut handshake).await?;
        remote.read_message(0).await?;
        Ok::<_, idevice::IdeviceError>(remote)
    })
    .await
    .map_err(|_| "DVT condition service connection timed out".to_string())?
    .map_err(|error| format!("DVT condition service unavailable: {error:?}"))
}

async fn timed_list<R: ReadWrite>(
    client: &mut ConditionInducerClient<'_, R>,
) -> Result<Vec<ConditionInducerGroup>, String> {
    tokio::time::timeout(OPERATION_TIMEOUT, client.available_conditions())
        .await
        .map_err(|_| "DVT condition listing timed out".to_string())?
        .map_err(|error| format!("DVT condition listing failed: {error:?}"))
}

async fn timed_apply_until<R: ReadWrite>(
    client: &mut ConditionInducerClient<'_, R>,
    selected: &ActiveDeviceCondition,
    deadline: tokio::time::Instant,
) -> Result<(), String> {
    tokio::time::timeout_at(
        deadline.min(tokio::time::Instant::now() + OPERATION_TIMEOUT),
        client.enable_condition(&selected.group_identifier, &selected.profile_identifier),
    )
    .await
    .map_err(|_| "DVT apply condition request expired".to_string())?
    .map_err(|error| format!("DVT apply condition failed: {error:?}"))
}

async fn timed_clear<R: ReadWrite>(
    client: &mut ConditionInducerClient<'_, R>,
) -> Result<(), String> {
    tokio::time::timeout(OPERATION_TIMEOUT, client.disable_condition())
        .await
        .map_err(|_| "DVT clear condition timed out".to_string())?
        .map_err(|error| format!("DVT clear condition failed: {error:?}"))
}

async fn timed_clear_until<R: ReadWrite>(
    client: &mut ConditionInducerClient<'_, R>,
    deadline: tokio::time::Instant,
) -> Result<(), String> {
    tokio::time::timeout_at(
        deadline.min(tokio::time::Instant::now() + OPERATION_TIMEOUT),
        client.disable_condition(),
    )
    .await
    .map_err(|_| "DVT clear condition request expired".to_string())?
    .map_err(|error| format!("DVT clear condition failed: {error:?}"))
}

fn ready_status(
    groups: &[DeviceConditionGroup],
    active: Option<ActiveDeviceCondition>,
) -> DeviceConditionStatus {
    DeviceConditionStatus {
        available: true,
        groups: groups.to_vec(),
        active,
        cleanup_pending: false,
        error: None,
    }
}

fn mark_failed(
    status: &DeviceConditionSlot,
    active: Option<ActiveDeviceCondition>,
    cleanup_pending: bool,
    error: String,
) {
    tracing::warn!(
        component = "device_conditions",
        operation = "failed",
        cleanup_pending,
        error = %error,
        "device condition backend stopped"
    );
    status.set(DeviceConditionStatus {
        active,
        cleanup_pending,
        error: Some(error),
        ..DeviceConditionStatus::default()
    });
}

fn normalize_groups(groups: Vec<ConditionInducerGroup>) -> Vec<DeviceConditionGroup> {
    let mut seen = HashSet::new();
    let mut remaining_profiles = MAX_PROFILES;
    groups
        .into_iter()
        .filter_map(|group| {
            if remaining_profiles == 0
                || validate_identifier(&group.identifier, "condition group").is_err()
            {
                return None;
            }
            let profiles = group
                .profiles
                .into_iter()
                .filter_map(|profile| {
                    if remaining_profiles == 0
                        || validate_identifier(&profile.identifier, "condition profile").is_err()
                        || !seen.insert((group.identifier.clone(), profile.identifier.clone()))
                    {
                        return None;
                    }
                    remaining_profiles -= 1;
                    Some(DeviceConditionProfile {
                        identifier: profile.identifier,
                        description: bounded_description(&profile.description),
                    })
                })
                .collect::<Vec<_>>();
            if profiles.is_empty() {
                None
            } else {
                Some(DeviceConditionGroup {
                    identifier: group.identifier,
                    profiles,
                })
            }
        })
        .take(MAX_GROUPS)
        .collect()
}

fn find_profile(
    groups: &[DeviceConditionGroup],
    group_identifier: &str,
    profile_identifier: &str,
) -> Option<ActiveDeviceCondition> {
    let group = groups
        .iter()
        .find(|group| group.identifier == group_identifier)?;
    let profile = group
        .profiles
        .iter()
        .find(|profile| profile.identifier == profile_identifier)?;
    Some(ActiveDeviceCondition {
        group_identifier: group.identifier.clone(),
        profile_identifier: profile.identifier.clone(),
        description: profile.description.clone(),
    })
}

fn validate_identifier(identifier: &str, label: &str) -> Result<(), String> {
    if identifier.is_empty()
        || identifier.len() > MAX_IDENTIFIER_BYTES
        || identifier.chars().any(char::is_control)
    {
        Err(format!("invalid {label} identifier"))
    } else {
        Ok(())
    }
}

fn bounded_description(description: &str) -> String {
    description
        .chars()
        .filter(|character| !character.is_control())
        .take(MAX_DESCRIPTION_CHARS)
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn normalizes_and_bounds_device_condition_catalog() {
        let groups = normalize_groups(vec![
            ConditionInducerGroup {
                identifier: "Network Link".into(),
                profiles: vec![
                    ConditionProfile {
                        identifier: "3G".into(),
                        description: "High latency\nprofile".into(),
                    },
                    ConditionProfile {
                        identifier: "3G".into(),
                        description: "duplicate".into(),
                    },
                ],
            },
            ConditionInducerGroup {
                identifier: "bad\nidentifier".into(),
                profiles: vec![ConditionProfile {
                    identifier: "ignored".into(),
                    description: "ignored".into(),
                }],
            },
        ]);
        assert_eq!(groups.len(), 1);
        assert_eq!(groups[0].profiles.len(), 1);
        assert_eq!(groups[0].profiles[0].description, "High latencyprofile");
    }

    #[test]
    fn resolves_only_an_enumerated_group_and_profile_pair() {
        let groups = vec![DeviceConditionGroup {
            identifier: "Network".into(),
            profiles: vec![DeviceConditionProfile {
                identifier: "LTE".into(),
                description: "LTE profile".into(),
            }],
        }];
        assert!(find_profile(&groups, "Network", "LTE").is_some());
        assert!(find_profile(&groups, "Thermal", "LTE").is_none());
        assert!(find_profile(&groups, "Network", "5G").is_none());
    }

    #[test]
    fn request_identifiers_are_bounded_and_single_line() {
        assert!(validate_identifiers("Network", "LTE").is_ok());
        assert!(validate_identifiers("", "LTE").is_err());
        assert!(validate_identifiers("Network", "bad\nprofile").is_err());
        assert!(validate_identifiers("x", &"p".repeat(MAX_IDENTIFIER_BYTES + 1)).is_err());
    }
}
