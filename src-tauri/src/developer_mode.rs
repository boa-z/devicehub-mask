use std::sync::Arc;
use std::time::Duration;

use idevice::services::amfi::AmfiClient;
use idevice::{IdeviceService, provider::IdeviceProvider};
use serde::Serialize;
use tokio::sync::oneshot;

const DEVELOPER_MODE_REQUEST_TIMEOUT: Duration = Duration::from_secs(6);

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub struct DeveloperModePreparation {
    pub already_enabled: bool,
}

#[derive(Debug)]
pub enum DeveloperModeCommand {
    RevealOption {
        reply: oneshot::Sender<Result<DeveloperModePreparation, String>>,
    },
}

pub fn execute(provider: Arc<dyn IdeviceProvider>, command: DeveloperModeCommand) {
    match command {
        DeveloperModeCommand::RevealOption { reply } => {
            tokio::spawn(async move {
                let result = tokio::time::timeout(
                    DEVELOPER_MODE_REQUEST_TIMEOUT,
                    reveal_developer_mode_option(provider.as_ref()),
                )
                .await
                .map_err(|_| "developer mode preparation timed out".to_string())
                .and_then(|result| result);
                let _ = reply.send(result);
            });
        }
    }
}

pub async fn read_status(provider: &dyn IdeviceProvider) -> Result<bool, String> {
    let mut client = AmfiClient::connect(provider)
        .await
        .map_err(|error| format!("unable to connect to the AMFI service: {error}"))?;
    client
        .get_developer_mode_status()
        .await
        .map_err(|error| format!("unable to verify Developer Mode status: {error}"))
}

async fn reveal_developer_mode_option(
    provider: &dyn IdeviceProvider,
) -> Result<DeveloperModePreparation, String> {
    let mut client = AmfiClient::connect(provider)
        .await
        .map_err(|error| format!("unable to connect to the AMFI service: {error}"))?;
    let already_enabled = client
        .get_developer_mode_status()
        .await
        .map_err(|error| format!("unable to verify Developer Mode status: {error}"))?;
    if already_enabled {
        tracing::info!("developer mode is already enabled; reveal request skipped");
        return Ok(DeveloperModePreparation {
            already_enabled: true,
        });
    }
    client
        .reveal_developer_mode_option_in_ui()
        .await
        .map_err(|error| format!("unable to reveal Developer Mode in Settings: {error}"))?;
    tracing::info!("requested Developer Mode option in device Settings");
    Ok(DeveloperModePreparation {
        already_enabled: false,
    })
}
