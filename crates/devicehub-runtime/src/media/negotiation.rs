//! DisplayService audio/video negotiation for a screen-control session.

use devicehub_core::{ConnKind, DeviceDetails};
use idevice::core_device::{
    CallInfoBlob, CoreDeviceError, DisplayServiceClient, build_screen_audio_offer,
    build_screen_video_offer, build_start_audio_parameters, build_start_video_parameters,
    parse_answer_media_blob,
};
use idevice::rsd::RsdHandshake;
use idevice::tcp::handle::{AdapterHandle, UdpSocketHandle};
use idevice::{IdeviceError, ReadWrite, RsdService};

/// `clientSupportedFeatures` advertised by the controller for screen sharing.
const CLIENT_SUPPORTED_FEATURES: u64 = 140;

/// Active DisplayService media session and its local RTP/RTCP sockets.
pub(crate) struct ScreenMediaStream {
    pub(crate) client: DisplayServiceClient<Box<dyn ReadWrite>>,
    pub(crate) audio_udp: UdpSocketHandle,
    pub(crate) video_udp: UdpSocketHandle,
    /// RFC 3550 video RTCP socket. `None` means the session relies on rtcp-mux.
    pub(crate) rtcp_udp: Option<UdpSocketHandle>,
}

/// Starts audio first to establish the shared session, then adds video using
/// the same `clientSessionID`.
pub(crate) async fn start_screen_media_stream(
    adapter: &mut AdapterHandle,
    handshake: &mut RsdHandshake,
    our_ssrc: u32,
    identity: Option<&DeviceDetails>,
    connection: ConnKind,
) -> Result<ScreenMediaStream, String> {
    let mut client = match DisplayServiceClient::connect_rsd(adapter, handshake).await {
        Ok(client) => client,
        Err(IdeviceError::ServiceNotFound) => {
            let mut related_services = handshake
                .services
                .keys()
                .filter(|name| {
                    let name = name.to_ascii_lowercase();
                    ["display", "screen", "media", "capture"]
                        .iter()
                        .any(|needle| name.contains(needle))
                })
                .cloned()
                .collect::<Vec<_>>();
            related_services.sort();
            tracing::warn!(
                connection = connection.label(),
                service_count = handshake.services.len(),
                ?related_services,
                "RSD did not advertise com.apple.coredevice.displayservice"
            );
            tracing::debug!(services = ?handshake.services.keys().collect::<Vec<_>>(), "RSD services");

            let hint = if cfg!(windows) {
                " USB supports displayservice, but this device has not published the Device Hub service set. Keep it connected and unlocked, then run `.\\scripts\\prepare-windows-device.ps1` to verify Developer Mode and mount the Personalized Developer Disk Image."
            } else {
                " The device has not published the Device Hub service set. Verify Developer Mode, the Personalized Developer Disk Image, and Device Hub pairing."
            };
            return Err(format!(
                "display service is unavailable on {} (RSD advertised {} services).{hint}",
                connection.label(),
                handshake.services.len()
            ));
        }
        Err(error) => return Err(format!("no display service: {error:?}")),
    };

    let audio_udp = adapter
        .bind_udp(0)
        .await
        .map_err(|error| format!("bind_udp(audio) failed: {error:?}"))?;
    let video_udp = adapter
        .bind_udp(0)
        .await
        .map_err(|error| format!("bind_udp(video) failed: {error:?}"))?;
    let receiver_ip = adapter.host_ip().to_string();
    let audio_receiver_port = audio_udp.local_port();
    let receiver_port = video_udp.local_port();
    let sender_ip = adapter.peer_ip().to_string();

    let rtcp_udp = adapter.bind_udp(receiver_port + 1).await.ok();
    if rtcp_udp.is_none() {
        tracing::info!(
            "RTCP port {} unavailable; relying on rtcp-mux",
            receiver_port + 1
        );
    }

    let call_info = call_info();
    let session_id = uuid::Uuid::new_v4();
    let audio_call_id = uuid::Uuid::new_v4().to_string().to_uppercase();
    let audio_offer = build_screen_audio_offer(&audio_call_id, &call_info)
        .map_err(|error| format!("audio offer build failed: {error:?}"))?;
    let audio_params = build_start_audio_parameters(
        &receiver_ip,
        audio_receiver_port,
        &sender_ip,
        50000,
        audio_offer,
        CLIENT_SUPPORTED_FEATURES,
        session_id,
    );
    let audio_response = client
        .start_media_stream(audio_params)
        .await
        .map_err(|error| format_media_start_error("audio", error, identity))?;
    log_audio_negotiation(&audio_response);

    start_video(
        &mut client,
        &receiver_ip,
        receiver_port,
        &sender_ip,
        session_id,
        our_ssrc,
        identity,
    )
    .await?;
    match client.get_media_stream_server_status().await {
        Ok(status) => log_media_server_status(&status),
        Err(error) => tracing::warn!(?error, "unable to query negotiated media stream status"),
    }

    Ok(ScreenMediaStream {
        client,
        audio_udp,
        video_udp,
        rtcp_udp,
    })
}

fn format_media_start_error(
    stream: &str,
    error: IdeviceError,
    identity: Option<&DeviceDetails>,
) -> String {
    let is_ios_27_gate = matches!(
        &error,
        IdeviceError::CoreDevice(CoreDeviceError::DeviceError(details))
            if details.contains("Integer(9021)")
                || details.contains("Remote control requires iOS 27.0 or later")
    );
    if !is_ios_27_gate {
        return format!("{stream} startMediaStream failed: {error:?}");
    }

    tracing::debug!(stream, error = ?error, "CoreDevice rejected remote-control capability");
    let detected = identity.map_or_else(
        || "this device".to_string(),
        |identity| {
            format!(
                "{} running iOS {}",
                identity.product_type, identity.product_version
            )
        },
    );
    format!(
        "Remote control is unavailable on {detected} (CoreDevice 9021). Apple requires iOS \
         27.0 or later for this device; update iOS or use a supported newer device. Switching \
         between USB and Wi-Fi cannot bypass this device-side capability check."
    )
}

fn log_audio_negotiation(response: &plist::Value) {
    let response_fields = response
        .as_dictionary()
        .map(|dictionary| dictionary.keys().cloned().collect::<Vec<_>>());
    let Some(answer) = find_negotiator_answer(response) else {
        tracing::warn!(
            ?response_fields,
            "audio negotiation response did not contain an answer"
        );
        tracing::debug!(response = ?response, "unparsed audio negotiation response");
        return;
    };
    let Ok(negotiation) = parse_answer_media_blob(answer) else {
        tracing::warn!(
            ?response_fields,
            answer_bytes = answer.len(),
            "unable to parse audio negotiation answer"
        );
        return;
    };
    tracing::info!(
        audio_features = negotiation
            .codec_features
            .as_ref()
            .map(|features| features.audio_features),
        stream_groups = negotiation.stream_groups.len(),
        "audio media negotiation accepted"
    );
    for (group_index, group) in negotiation.stream_groups.iter().enumerate() {
        for payload in &group.payloads {
            tracing::info!(
                group_index,
                stream_group = group.stream_group,
                codec_type = payload.codec_type,
                rtp_payload_type = payload.rtp_payload,
                packet_time = payload.p_time,
                rtcp_flags = payload.rtcp_flags,
                media_flags = payload.media_flags,
                profile_level_id = payload.profile_level_id,
                rtp_sample_rate = payload.rtp_sample_rate,
                cipher_suite = payload.cipher_suite,
                packed_payload_bytes = payload.packed_payload.len(),
                encoder_usage = payload.encoder_usage,
                "negotiated audio payload"
            );
        }
        for stream in &group.streams {
            tracing::info!(
                group_index,
                stream_group = group.stream_group,
                rtp_ssrc = format_args!("{:#x}", stream.rtp_ssrc),
                stream_id = stream.stream_id,
                audio_channels = stream.audio_channel_count,
                stream_index = stream.stream_index,
                required_payload_bytes = stream.required_packed_payload.len(),
                optional_payload_bytes = stream.optional_packed_payload.len(),
                "negotiated audio stream"
            );
        }
    }
}

fn log_media_server_status(status: &plist::Value) {
    let mut fields = Vec::new();
    collect_plist_fields("media_status", status, &mut fields, 0);
    tracing::info!(
        fields = fields.len(),
        "captured negotiated media stream status"
    );
    for (path, value) in fields.into_iter().take(256) {
        tracing::debug!(target: "devicehub_mask::audio", %path, %value, "media stream status field");
    }
}

fn collect_plist_fields(
    path: &str,
    value: &plist::Value,
    fields: &mut Vec<(String, String)>,
    depth: usize,
) {
    if depth > 10 || fields.len() >= 256 {
        return;
    }
    match value {
        plist::Value::Dictionary(dictionary) => {
            for (key, value) in dictionary {
                collect_plist_fields(&format!("{path}.{key}"), value, fields, depth + 1);
            }
        }
        plist::Value::Array(values) => {
            for (index, value) in values.iter().enumerate() {
                collect_plist_fields(&format!("{path}[{index}]"), value, fields, depth + 1);
            }
        }
        plist::Value::Data(data) => {
            fields.push((path.to_string(), format!("data[{}]", data.len())));
            if let Ok(nested) = plist::from_bytes::<plist::Value>(data) {
                collect_plist_fields(&format!("{path}.plist"), &nested, fields, depth + 1);
            }
        }
        plist::Value::String(value) => {
            let normalized_path = path.to_ascii_lowercase();
            let sensitive = ["address", "ip", "uuid", "sessionid", "deviceid"]
                .iter()
                .any(|key| normalized_path.contains(key));
            let value = if sensitive {
                "<redacted>".to_string()
            } else {
                value.chars().take(160).collect()
            };
            fields.push((path.to_string(), value));
        }
        plist::Value::Boolean(value) => fields.push((path.to_string(), value.to_string())),
        plist::Value::Real(value) => fields.push((path.to_string(), value.to_string())),
        plist::Value::Integer(value) => fields.push((path.to_string(), format!("{value:?}"))),
        plist::Value::Date(_) => fields.push((path.to_string(), "<date>".into())),
        plist::Value::Uid(value) => fields.push((path.to_string(), format!("{value:?}"))),
        _ => fields.push((path.to_string(), format!("{value:?}"))),
    }
}

fn find_negotiator_answer(value: &plist::Value) -> Option<&[u8]> {
    match value {
        plist::Value::Dictionary(dictionary) => dictionary.iter().find_map(|(key, value)| {
            if key.to_ascii_lowercase().contains("negotiatoranswer") {
                value.as_data()
            } else {
                find_negotiator_answer(value)
            }
        }),
        plist::Value::Array(values) => values.iter().find_map(find_negotiator_answer),
        _ => None,
    }
}

fn call_info() -> CallInfoBlob {
    CallInfoBlob {
        call_id: 0,
        client_version: 1,
        device_type: "Mac17,7".into(),
        framework_version: "2205.3.1".into(),
        os_version: "25F71".into(),
        device_name: None,
        audio_device_uid: None,
    }
}

async fn start_video(
    client: &mut DisplayServiceClient<Box<dyn ReadWrite>>,
    receiver_ip: &str,
    receiver_port: u16,
    sender_ip: &str,
    session_id: uuid::Uuid,
    our_ssrc: u32,
    identity: Option<&DeviceDetails>,
) -> Result<(), String> {
    let call_id = uuid::Uuid::new_v4().to_string().to_uppercase();
    let offer = build_screen_video_offer(&call_id, &call_info(), our_ssrc)
        .map_err(|error| format!("video offer build failed: {error:?}"))?;
    let params = build_start_video_parameters(
        receiver_ip,
        receiver_port,
        sender_ip,
        50001,
        offer,
        CLIENT_SUPPORTED_FEATURES,
        1,
        session_id,
    );
    client
        .start_media_stream(params)
        .await
        .map_err(|error| format_media_start_error("video", error, identity))?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn summarizes_coredevice_9021_without_binary_plist_dump() {
        let error = IdeviceError::CoreDevice(CoreDeviceError::DeviceError(
            r#"Dictionary({"code": Integer(9021), "NSLocalizedDescription": String("Remote control requires iOS 27.0 or later on this device.")})"#.into(),
        ));
        let identity = DeviceDetails {
            udid: "phone".into(),
            name: "Test iPhone".into(),
            product_type: "iPhone11,2".into(),
            product_version: "26.0".into(),
            build_version: None,
            device_class: None,
            cpu_architecture: None,
            model_number: None,
            hardware_model: None,
            device_color: None,
            enclosure_color: None,
            serial_number: None,
            ecid: None,
            total_disk_capacity: None,
            storage: None,
            activation_state: None,
            developer_mode_enabled: None,
            developer_image_mounted: None,
            regional_settings: None,
            battery: None,
        };

        let message = format_media_start_error("audio", error, Some(&identity));
        assert!(message.contains("CoreDevice 9021"));
        assert!(message.contains("iPhone11,2 running iOS 26.0"));
        assert!(message.contains("iOS 27.0 or later"));
        assert!(!message.contains("Dictionary"));
    }
}
