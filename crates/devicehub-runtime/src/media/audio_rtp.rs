//! AAC-ELD RTP validation, diagnostics, and RFC 3640 packetization.

use std::net::SocketAddr;
use std::time::{Duration, Instant};

use idevice::core_device::{RtpPacket, is_rtcp};
use idevice::tcp::handle::UdpSocketHandle;

use super::session::RunningStats;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct AacAuHeader {
    header_bits: u16,
    access_units: u16,
    first_access_unit_bytes: u16,
}

/// Drains negotiated audio RTP, optionally forwarding RFC 3640 packets to a
/// local decoder socket while retaining bounded transport diagnostics.
pub(crate) async fn receive_audio_rtp(
    udp: &UdpSocketHandle,
    forwarding: Option<(&tokio::net::UdpSocket, SocketAddr)>,
) {
    let mut stream: Option<(u8, u32)> = None;
    let mut last_sequence = None;
    let mut last_timestamp = None;
    let mut timestamp_deltas = RunningStats::default();
    let mut payload_sizes = RunningStats::default();
    let mut packets = 0_u64;
    let mut bytes = 0_u64;
    let mut lost_packets = 0_u64;
    let mut marker_packets = 0_u64;
    let mut rtcp_packets = 0_u64;
    let mut started = Instant::now();
    loop {
        let datagram = match udp.recv().await {
            Ok(datagram) => datagram,
            Err(error) => {
                tracing::warn!(?error, "audio UDP receive failed");
                return;
            }
        };
        if is_rtcp(&datagram.data) {
            rtcp_packets += 1;
            continue;
        }
        let Some(packet) = RtpPacket::parse(&datagram.data) else {
            continue;
        };
        if let Some((sender, target)) = forwarding {
            match add_rfc3640_au_header(&datagram.data) {
                Ok(packet) => {
                    if let Err(error) = sender.send_to(&packet, target).await {
                        tracing::warn!(%error, "failed to forward audio RTP packet to local decoder");
                        return;
                    }
                }
                Err(error) => {
                    tracing::warn!(
                        error,
                        packet_bytes = datagram.data.len(),
                        "dropping invalid audio RTP packet"
                    );
                    continue;
                }
            }
        }
        if stream != Some((packet.payload_type, packet.ssrc)) {
            stream = Some((packet.payload_type, packet.ssrc));
            last_sequence = None;
            last_timestamp = None;
            tracing::info!(
                rtp_payload_type = packet.payload_type,
                rtp_ssrc = format_args!("{:#x}", packet.ssrc),
                source_port = datagram.source_port,
                extension = packet.extension,
                extension_profile = format_args!("{:#x}", packet.ext_profile),
                extension_bytes = packet.ext_data.len(),
                payload_bytes = packet.payload.len(),
                aac_au_header = ?parse_aac_au_header(packet.payload),
                "audio RTP stream detected"
            );
        }
        if let Some(previous) = last_sequence {
            let distance = packet.sequence_number.wrapping_sub(previous);
            if distance > 1 && distance < 0x8000 {
                lost_packets += u64::from(distance - 1);
            }
        }
        if let Some(previous) = last_timestamp {
            let delta = packet.timestamp.wrapping_sub(previous);
            if delta > 0 && delta < 1_000_000 {
                timestamp_deltas.push(delta as f64);
            }
        }
        last_sequence = Some(packet.sequence_number);
        last_timestamp = Some(packet.timestamp);
        packets += 1;
        bytes += datagram.data.len() as u64;
        marker_packets += u64::from(packet.marker);
        payload_sizes.push(packet.payload.len() as f64);

        if started.elapsed() >= Duration::from_secs(5) {
            tracing::debug!(
                target: "devicehub_mask::audio",
                elapsed_ms = started.elapsed().as_millis() as u64,
                packets,
                bytes,
                lost_packets,
                marker_packets,
                rtcp_packets,
                payload_bytes_mean = ?payload_sizes.mean(),
                payload_bytes_min = ?payload_sizes.min(),
                payload_bytes_max = ?payload_sizes.max(),
                timestamp_delta_ticks = ?timestamp_deltas.mean(),
                timestamp_delta_min_ticks = ?timestamp_deltas.min(),
                timestamp_delta_max_ticks = ?timestamp_deltas.max(),
                "audio RTP diagnostics"
            );
            packets = 0;
            bytes = 0;
            lost_packets = 0;
            marker_packets = 0;
            rtcp_packets = 0;
            payload_sizes = RunningStats::default();
            timestamp_deltas = RunningStats::default();
            started = Instant::now();
        }
    }
}

fn parse_aac_au_header(payload: &[u8]) -> Option<AacAuHeader> {
    let header_bits = u16::from_be_bytes([*payload.first()?, *payload.get(1)?]);
    if header_bits == 0 || header_bits % 16 != 0 {
        return None;
    }
    let header_bytes = usize::from(header_bits).div_ceil(8);
    if payload.len() < 2 + header_bytes || header_bytes < 2 {
        return None;
    }
    let first = u16::from_be_bytes([payload[2], payload[3]]);
    let first_access_unit_bytes = first >> 3;
    let encoded_bytes = payload.len() - 2 - header_bytes;
    if usize::from(first_access_unit_bytes) > encoded_bytes {
        return None;
    }
    Some(AacAuHeader {
        header_bits,
        access_units: header_bits / 16,
        first_access_unit_bytes,
    })
}

fn add_rfc3640_au_header(packet: &[u8]) -> Result<Vec<u8>, &'static str> {
    if packet.len() < 12 || packet[0] >> 6 != 2 {
        return Err("invalid RTP header");
    }
    let csrc_bytes = usize::from(packet[0] & 0x0f)
        .checked_mul(4)
        .ok_or("RTP header overflow")?;
    let mut payload_offset = 12_usize
        .checked_add(csrc_bytes)
        .ok_or("RTP header overflow")?;
    if packet.len() < payload_offset {
        return Err("truncated RTP CSRC list");
    }
    if packet[0] & 0x10 != 0 {
        if packet.len() < payload_offset + 4 {
            return Err("truncated RTP extension header");
        }
        let extension_words =
            u16::from_be_bytes([packet[payload_offset + 2], packet[payload_offset + 3]]);
        payload_offset = payload_offset
            .checked_add(4 + usize::from(extension_words) * 4)
            .ok_or("RTP extension overflow")?;
        if packet.len() < payload_offset {
            return Err("truncated RTP extension");
        }
    }
    let mut payload_end = packet.len();
    if packet[0] & 0x20 != 0 {
        let padding = usize::from(*packet.last().ok_or("missing RTP padding")?);
        if padding == 0 || padding > payload_end.saturating_sub(payload_offset) {
            return Err("invalid RTP padding");
        }
        payload_end -= padding;
    }
    let payload_len = payload_end.saturating_sub(payload_offset);
    if payload_len == 0 || payload_len > 0x1fff {
        return Err("AAC access unit length is outside the 13-bit RFC 3640 range");
    }
    let mut adapted = Vec::with_capacity(payload_offset + 4 + payload_len);
    adapted.extend_from_slice(&packet[..payload_offset]);
    adapted[0] &= !0x20;
    adapted.extend_from_slice(&[0, 16]);
    adapted.extend_from_slice(&((payload_len as u16) << 3).to_be_bytes());
    adapted.extend_from_slice(&packet[payload_offset..payload_end]);
    Ok(adapted)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn recognizes_rfc3640_aac_access_unit_headers_without_reading_audio_data() {
        let payload = [0x00, 0x10, 0x00, 0x20, 1, 2, 3, 4];
        assert_eq!(
            parse_aac_au_header(&payload),
            Some(AacAuHeader {
                header_bits: 16,
                access_units: 1,
                first_access_unit_bytes: 4,
            })
        );
        assert_eq!(parse_aac_au_header(&[0x00, 0x10, 0x01, 0x00, 1]), None);
        assert_eq!(parse_aac_au_header(&[0x00, 0x07, 0, 0]), None);
    }

    #[test]
    fn adds_rfc3640_header_to_raw_aac_rtp() {
        let mut packet = vec![0x80, 101, 0, 1, 0, 0, 1, 224, 1, 2, 3, 4];
        packet.extend_from_slice(&[0xaa, 0xbb, 0xcc]);
        let adapted = add_rfc3640_au_header(&packet).unwrap();
        assert_eq!(&adapted[..12], &packet[..12]);
        assert_eq!(&adapted[12..16], &[0, 16, 0, 24]);
        assert_eq!(&adapted[16..], &[0xaa, 0xbb, 0xcc]);
    }

    #[test]
    fn preserves_rtp_extensions_and_removes_padding() {
        let mut packet = vec![0xb1, 101, 0, 1, 0, 0, 1, 224, 1, 2, 3, 4];
        packet.extend_from_slice(&[9, 8, 7, 6]);
        packet.extend_from_slice(&[0xbe, 0xde, 0, 1, 1, 2, 3, 4]);
        packet.extend_from_slice(&[0xaa, 0xbb, 0, 0, 3]);
        let adapted = add_rfc3640_au_header(&packet).unwrap();
        assert_eq!(adapted[0], 0x91);
        assert_eq!(
            &adapted[..24],
            &[
                0x91, 101, 0, 1, 0, 0, 1, 224, 1, 2, 3, 4, 9, 8, 7, 6, 0xbe, 0xde, 0, 1, 1, 2, 3,
                4,
            ]
        );
        assert_eq!(&adapted[24..], &[0, 16, 0, 16, 0xaa, 0xbb]);
    }

    #[test]
    fn rejects_oversized_or_truncated_audio_rtp() {
        let mut oversized = vec![0x80, 101, 0, 1, 0, 0, 1, 224, 1, 2, 3, 4];
        oversized.resize(12 + 0x2000, 0);
        assert!(add_rfc3640_au_header(&oversized).is_err());
        assert!(add_rfc3640_au_header(&[0x90, 101, 0, 1, 0, 0, 1, 224, 1, 2, 3, 4]).is_err());
    }
}
