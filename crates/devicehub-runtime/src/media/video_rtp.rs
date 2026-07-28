//! HEVC RTP ingestion for the screen-media session.

use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use devicehub_core::VideoCounters;
use idevice::core_device::{HevcDepacketizer, RtpPacket, build_frame_ack, is_rtcp};
use idevice::tcp::handle::UdpSocketHandle;
use tokio::sync::{Notify, mpsc};

use super::rtcp::RtcpShared;
use super::session::{
    AccessUnitAssembler, HEVC_QUEUE_MAX_BYTES, HevcQueue, HevcQueuePush, RunningStats,
};
use crate::Demand;

/// How long the selected RTP stream must be quiet before a new SSRC can take over.
const SSRC_TAKEOVER_GRACE: Duration = Duration::from_millis(250);

/// Optional diagnostics for video ingestion. Defaults preserve the production
/// media path: no experimental frame ACK and no Annex-B copy.
#[derive(Default)]
pub(crate) struct VideoRtpOptions {
    pub(crate) send_frame_ack: bool,
    pub(crate) annexb_sink: Option<mpsc::Sender<Vec<u8>>>,
    pub(crate) demand: Demand,
}

/// Receives video RTP, depacketizes HEVC, and queues complete Annex-B access
/// units. The optional sink is diagnostic-only and must not apply backpressure
/// to the real-time media path.
pub(crate) async fn receive_video_rtp(
    udp: Arc<UdpSocketHandle>,
    hevc_queue: Arc<HevcQueue>,
    rtcp: Arc<Mutex<RtcpShared>>,
    corruption: Arc<Notify>,
    video_counters: VideoCounters,
    our_ssrc: u32,
    options: VideoRtpOptions,
) {
    let mut depacketizer = HevcDepacketizer::new();
    let mut assembler = AccessUnitAssembler::default();
    // Old senders can linger after a stream restart. Keep one SSRC selected
    // until it is genuinely quiet instead of mixing two HEVC packet streams.
    let mut locked_ssrc: Option<u32> = None;
    let mut last_locked = Instant::now();
    let mut prev_marker_seq: Option<u16> = None;
    let mut au_pkts: u32 = 0;
    let mut metrics_started = Instant::now();
    let mut metrics_rtp_packets = 0_u64;
    let mut metrics_rtp_bytes = 0_u64;
    let mut metrics_access_units = 0_u64;
    let mut metrics_hevc_bytes = 0_u64;
    let mut metrics_incomplete_markers = 0_u64;
    let mut last_rtp_frame_timestamp = None;
    let mut last_source_frame_at = None;
    let mut rtp_timestamp_deltas = RunningStats::default();
    let mut source_frame_intervals_ms = RunningStats::default();
    let mut dump_backpressured = false;
    let mut was_demanded = false;

    loop {
        match udp.recv().await {
            Ok(datagram) => {
                let now = Instant::now();
                video_counters.note_transport_activity();
                if is_rtcp(&datagram.data) {
                    rtcp.lock().unwrap().note_inbound(
                        &datagram.data,
                        datagram.source_port,
                        false,
                        now,
                    );
                    continue;
                }
                let Some(packet) = RtpPacket::parse(&datagram.data) else {
                    continue;
                };
                let demanded = options.demand.enabled();
                if demanded != was_demanded {
                    depacketizer = HevcDepacketizer::new();
                    assembler.clear();
                    prev_marker_seq = None;
                    au_pkts = 0;
                    let (dropped_access_units, dropped_bytes) = hevc_queue.force_resync();
                    tracing::debug!(
                        demanded,
                        dropped_access_units,
                        dropped_bytes,
                        "updated device video resource demand"
                    );
                    if demanded {
                        corruption.notify_one();
                    }
                    was_demanded = demanded;
                }
                if starts_irap(packet.payload) {
                    tracing::info!(
                        rtp_ssrc = format_args!("{:#x}", packet.ssrc),
                        "received IRAP keyframe"
                    );
                }
                match locked_ssrc {
                    Some(ssrc) if ssrc == packet.ssrc => last_locked = now,
                    Some(ssrc) => {
                        if now.duration_since(last_locked) < SSRC_TAKEOVER_GRACE {
                            continue;
                        }
                        tracing::info!(
                            old_rtp_ssrc = format_args!("{ssrc:#x}"),
                            new_rtp_ssrc = format_args!("{:#x}", packet.ssrc),
                            "RTP stream went quiet; migrating"
                        );
                        depacketizer = HevcDepacketizer::new();
                        assembler.clear();
                        prev_marker_seq = None;
                        au_pkts = 0;
                        last_rtp_frame_timestamp = None;
                        last_source_frame_at = None;
                        rtp_timestamp_deltas = RunningStats::default();
                        source_frame_intervals_ms = RunningStats::default();
                        let (dropped_access_units, dropped_bytes) = hevc_queue.force_resync();
                        tracing::info!(
                            dropped_access_units,
                            dropped_bytes,
                            "cleared HEVC queue for RTP stream migration"
                        );
                        locked_ssrc = Some(packet.ssrc);
                        last_locked = now;
                        rtcp.lock().unwrap().reset_media_source(packet.ssrc);
                    }
                    None => {
                        locked_ssrc = Some(packet.ssrc);
                        last_locked = now;
                    }
                }
                metrics_rtp_packets += 1;
                metrics_rtp_bytes += datagram.data.len() as u64;
                rtcp.lock().unwrap().note_rtp_packet(
                    packet.ssrc,
                    packet.sequence_number,
                    packet.marker,
                );

                if !demanded {
                    if packet.marker {
                        video_counters.note_source_frame();
                    }
                    continue;
                }

                let belongs_to_current_au = prev_marker_seq.is_none_or(|previous| {
                    let distance = packet.sequence_number.wrapping_sub(previous);
                    distance != 0 && distance < 0x8000
                });
                if belongs_to_current_au {
                    au_pkts = au_pkts.wrapping_add(1);
                }
                let complete_access_unit = if packet.marker {
                    video_counters.note_source_frame();
                    if let Some(previous) = last_rtp_frame_timestamp {
                        let delta = packet.timestamp.wrapping_sub(previous);
                        if delta > 0 && delta <= 1_000_000 {
                            rtp_timestamp_deltas.push(delta as f64);
                        }
                    }
                    last_rtp_frame_timestamp = Some(packet.timestamp);
                    if let Some(previous) = last_source_frame_at {
                        source_frame_intervals_ms
                            .push(now.duration_since(previous).as_secs_f64() * 1000.0);
                    }
                    last_source_frame_at = Some(now);
                    let complete = match prev_marker_seq {
                        Some(previous) => {
                            let expected = packet.sequence_number.wrapping_sub(previous) as u32;
                            au_pkts >= expected
                        }
                        None => true,
                    };
                    if options.send_frame_ack && complete {
                        let ack = build_frame_ack(our_ssrc, packet.timestamp);
                        udp.send_to(datagram.source_port, ack).await.ok();
                    }
                    prev_marker_seq = Some(packet.sequence_number);
                    au_pkts = 0;
                    if !complete {
                        metrics_incomplete_markers += 1;
                    }
                    complete
                } else {
                    false
                };

                depacketizer.push(packet.sequence_number, packet.timestamp, packet.payload);
                let output = depacketizer.take_output();
                if !output.is_empty() {
                    if let Some(sink) = &options.annexb_sink {
                        match sink.try_send(output.clone()) {
                            Ok(()) => dump_backpressured = false,
                            Err(mpsc::error::TrySendError::Full(_)) if !dump_backpressured => {
                                dump_backpressured = true;
                                tracing::warn!(
                                    "HEVC diagnostic sink is behind; dump chunks will be dropped"
                                );
                            }
                            Err(mpsc::error::TrySendError::Closed(_)) => {}
                            Err(mpsc::error::TrySendError::Full(_)) => {}
                        }
                    }
                    let mut access_units = assembler.push(&output, packet.timestamp);
                    if complete_access_unit && let Some(access_unit) = assembler.finish() {
                        access_units.push(access_unit);
                    }
                    for access_unit in access_units {
                        metrics_access_units += 1;
                        metrics_hevc_bytes += access_unit.bytes.len() as u64;
                        match hevc_queue.push(access_unit) {
                            HevcQueuePush::Enqueued | HevcQueuePush::Dropped => {}
                            HevcQueuePush::NeedsKeyframe {
                                queued_bytes,
                                incoming_bytes,
                            } => {
                                tracing::warn!(
                                    queue_limit_bytes = HEVC_QUEUE_MAX_BYTES,
                                    queued_bytes,
                                    incoming_bytes,
                                    "HEVC queue overflow; dropping until IRAP"
                                );
                                corruption.notify_one();
                            }
                            HevcQueuePush::Recovered {
                                dropped_access_units,
                                dropped_bytes,
                            } => {
                                tracing::info!(
                                    dropped_access_units,
                                    dropped_bytes,
                                    "HEVC queue resumed at IRAP"
                                );
                            }
                        }
                    }
                }
                if metrics_started.elapsed() >= Duration::from_secs(5) {
                    let elapsed_ms = metrics_started.elapsed().as_millis() as u64;
                    let queue = hevc_queue.take_snapshot();
                    let source_fps = source_frame_intervals_ms
                        .mean()
                        .filter(|interval| *interval > 0.0)
                        .map(|interval| 1000.0 / interval);
                    tracing::debug!(
                        target: "devicehub_mask::perf",
                        elapsed_ms,
                        rtp_packets = metrics_rtp_packets,
                        rtp_bytes = metrics_rtp_bytes,
                        access_units = metrics_access_units,
                        hevc_bytes = metrics_hevc_bytes,
                        incomplete_markers = metrics_incomplete_markers,
                        ?source_fps,
                        source_frame_interval_ms = ?source_frame_intervals_ms.mean(),
                        source_frame_interval_min_ms = ?source_frame_intervals_ms.min(),
                        source_frame_interval_max_ms = ?source_frame_intervals_ms.max(),
                        source_frame_jitter_ms = ?source_frame_intervals_ms.standard_deviation(),
                        rtp_timestamp_delta_ticks = ?rtp_timestamp_deltas.mean(),
                        rtp_timestamp_delta_min_ticks = ?rtp_timestamp_deltas.min(),
                        rtp_timestamp_delta_max_ticks = ?rtp_timestamp_deltas.max(),
                        queue_access_units = queue.queued_access_units,
                        queue_bytes = queue.queued_bytes,
                        queue_peak_bytes = queue.peak_bytes,
                        waiting_for_irap = queue.waiting_for_irap,
                        queue_wait_ms = queue.wait_ms,
                        queue_wait_max_ms = queue.wait_max_ms,
                        "video input performance"
                    );
                    metrics_started = Instant::now();
                    metrics_rtp_packets = 0;
                    metrics_rtp_bytes = 0;
                    metrics_access_units = 0;
                    metrics_hevc_bytes = 0;
                    metrics_incomplete_markers = 0;
                    rtp_timestamp_deltas = RunningStats::default();
                    source_frame_intervals_ms = RunningStats::default();
                }
            }
            Err(error) => {
                tracing::warn!(?error, "video UDP receive stopped");
                break;
            }
        }
    }
    hevc_queue.close();
}

fn starts_irap(payload: &[u8]) -> bool {
    if payload.len() >= 3 && (payload[0] >> 1) & 0x3f == 49 {
        (payload[2] & 0x80) != 0 && (16..=23).contains(&(payload[2] & 0x3f))
    } else if payload.len() >= 2 {
        (16..=23).contains(&((payload[0] >> 1) & 0x3f))
    } else {
        false
    }
}

#[cfg(test)]
mod tests {
    use super::starts_irap;

    #[test]
    fn recognizes_complete_and_fragmented_irap_starts() {
        assert!(starts_irap(&[19 << 1, 1]));
        assert!(starts_irap(&[49 << 1, 1, 0x80 | 19]));
        assert!(!starts_irap(&[49 << 1, 1, 19]));
        assert!(!starts_irap(&[1 << 1, 1]));
        assert!(!starts_irap(&[]));
    }
}
