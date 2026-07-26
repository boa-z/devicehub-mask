use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use idevice::core_device::{
    ReportBlock, SenderReport, build_keyframe_request, build_liveness, build_rctl, is_rtcp,
};
use idevice::tcp::handle::UdpSocketHandle;
use tokio::sync::Notify;

use devicehub_core::VideoCounters;

const KEYFRAME_DEBOUNCE: Duration = Duration::from_millis(1500);
/// AVConference uses periodic Receiver Reports for liveness.
const REPORT_INTERVAL: Duration = Duration::from_secs(1);
/// Default destination until the device's RTCP source port is learned.
const VIDEO_SENDER_PORT: u16 = 50001;

/// Where the device's RTCP arrives. Until learned, feedback covers both
/// rtcp-mux and the separate RFC 3550 port.
#[derive(Debug, Clone, Copy, Default)]
enum RtcpPeer {
    #[default]
    Unknown,
    Mux(u16),
    Separate(u16),
}

#[derive(Debug, Clone, Copy)]
struct SenderReportEcho {
    ntp_middle: u32,
    received_at: Instant,
}

#[derive(Debug, Default)]
struct ReceptionStats {
    initialized: bool,
    base_seq: u32,
    ext_max: u32,
    received: u32,
    expected_prior: u32,
    received_prior: u32,
}

impl ReceptionStats {
    fn on_packet(&mut self, sequence: u16) {
        let sequence = u32::from(sequence);
        if !self.initialized {
            self.initialized = true;
            self.base_seq = sequence;
            self.ext_max = sequence;
            self.received = 1;
            return;
        }
        let cycles = self.ext_max & !0xffff;
        let max_low = self.ext_max & 0xffff;
        let extended = if sequence >= max_low {
            if sequence - max_low < 0x8000 {
                cycles | sequence
            } else {
                cycles.wrapping_sub(0x10000) | sequence
            }
        } else if max_low - sequence < 0x8000 {
            cycles | sequence
        } else {
            (cycles + 0x10000) | sequence
        };
        if extended > self.ext_max {
            self.ext_max = extended;
        }
        self.received = self.received.wrapping_add(1);
    }

    fn highest_sequence_relative(&self) -> u16 {
        if self.initialized {
            self.ext_max.wrapping_sub(self.base_seq) as u16
        } else {
            0
        }
    }

    fn report_block(&mut self, source_ssrc: u32, lsr: u32, dlsr: u32) -> ReportBlock {
        let expected = self.ext_max.wrapping_sub(self.base_seq).wrapping_add(1);
        let cumulative_lost = expected.saturating_sub(self.received);
        let expected_interval = expected.wrapping_sub(self.expected_prior);
        let received_interval = self.received.wrapping_sub(self.received_prior);
        self.expected_prior = expected;
        self.received_prior = self.received;
        let lost_interval = expected_interval.saturating_sub(received_interval);
        let fraction_lost = if expected_interval == 0 || lost_interval == 0 {
            0
        } else {
            ((lost_interval << 8) / expected_interval) as u8
        };
        ReportBlock {
            source_ssrc,
            fraction_lost,
            cumulative_lost: cumulative_lost & 0x00ff_ffff,
            highest_seq: self.ext_max,
            jitter: 0,
            lsr,
            dlsr,
        }
    }
}

/// State shared by RTP ingest and RTCP receive/send tasks.
#[derive(Default)]
pub struct RtcpShared {
    media_ssrc: Option<u32>,
    stats: ReceptionStats,
    sender_report: Option<SenderReportEcho>,
    peer: RtcpPeer,
    frames: u32,
}

impl RtcpShared {
    pub fn note_inbound(&mut self, bytes: &[u8], source_port: u16, separate: bool, now: Instant) {
        self.peer = if separate {
            RtcpPeer::Separate(source_port)
        } else {
            RtcpPeer::Mux(source_port)
        };
        if let Some(report) = SenderReport::parse_first(bytes) {
            self.sender_report = Some(SenderReportEcho {
                ntp_middle: report.ntp_middle,
                received_at: now,
            });
            self.media_ssrc.get_or_insert(report.ssrc);
        }
    }

    pub fn reset_media_source(&mut self, ssrc: u32) {
        self.media_ssrc = Some(ssrc);
        self.stats = ReceptionStats::default();
    }

    pub fn note_rtp_packet(&mut self, ssrc: u32, sequence: u16, marker: bool) {
        self.media_ssrc.get_or_insert(ssrc);
        self.stats.on_packet(sequence);
        if marker {
            self.frames = self.frames.wrapping_add(1);
        }
    }

    fn report_blocks(&mut self, now: Instant) -> Vec<ReportBlock> {
        let Some(ssrc) = self.media_ssrc else {
            return Vec::new();
        };
        let (lsr, dlsr) = match self.sender_report {
            Some(report) => {
                let delay = now.saturating_duration_since(report.received_at);
                (report.ntp_middle, (delay.as_secs_f64() * 65536.0) as u32)
            }
            None => (0, 0),
        };
        vec![self.stats.report_block(ssrc, lsr, dlsr)]
    }
}

/// Receives RTCP from the separate RFC 3550 socket. A missing socket is a valid
/// rtcp-mux configuration and therefore remains pending for the session lifetime.
pub async fn receive_task(
    udp: Option<Arc<UdpSocketHandle>>,
    state: Arc<Mutex<RtcpShared>>,
    counters: VideoCounters,
) {
    let Some(udp) = udp else {
        std::future::pending::<()>().await;
        return;
    };
    loop {
        match udp.recv().await {
            Ok(datagram) => {
                if is_rtcp(&datagram.data) {
                    counters.note_transport_activity();
                    state.lock().unwrap().note_inbound(
                        &datagram.data,
                        datagram.source_port,
                        true,
                        Instant::now(),
                    );
                }
            }
            Err(error) => {
                tracing::warn!(%error, "RTCP UDP receive stopped");
                break;
            }
        }
    }
}

/// Sends liveness reports and corruption-triggered keyframe requests to the
/// learned peer without exposing RTCP packet construction to session orchestration.
pub async fn send_task(
    rtp_udp: Arc<UdpSocketHandle>,
    rtcp_udp: Option<Arc<UdpSocketHandle>>,
    state: Arc<Mutex<RtcpShared>>,
    our_ssrc: u32,
    cname: String,
    corruption: &Notify,
) {
    let send = |peer: RtcpPeer, packet: Vec<u8>| {
        let rtp_udp = rtp_udp.clone();
        let rtcp_udp = rtcp_udp.clone();
        async move {
            match peer {
                RtcpPeer::Mux(port) => {
                    rtp_udp.send_to(port, packet).await.ok();
                }
                RtcpPeer::Separate(port) => {
                    if let Some(socket) = &rtcp_udp {
                        socket.send_to(port, packet).await.ok();
                    }
                }
                RtcpPeer::Unknown => {
                    rtp_udp
                        .send_to(VIDEO_SENDER_PORT, packet.clone())
                        .await
                        .ok();
                    if let Some(socket) = &rtcp_udp {
                        socket.send_to(VIDEO_SENDER_PORT + 1, packet).await.ok();
                    }
                }
            }
        }
    };

    let mut fir_sequence = 0_u8;
    let started_at = Instant::now();
    // Experimental RCTL remains opt-in because the packet is not yet known to be
    // byte-correct and has previously desynchronized the encoder.
    let send_rctl = std::env::var("DEVICEHUB_RCTL").is_ok();
    let mut report_tick = tokio::time::interval(REPORT_INTERVAL);
    let mut rctl_tick = tokio::time::interval(Duration::from_millis(50));
    loop {
        tokio::select! {
            _ = rctl_tick.tick() => {
                if !send_rctl {
                    continue;
                }
                let built = {
                    let state = state.lock().unwrap();
                    state.media_ssrc.map(|_| {
                        let clock_ms = started_at.elapsed().as_millis() as u16;
                        let packet = build_rctl(
                            our_ssrc,
                            clock_ms,
                            state.frames as u16,
                            state.stats.highest_sequence_relative(),
                        );
                        (state.peer, packet)
                    })
                };
                if let Some((peer, packet)) = built {
                    send(peer, packet).await;
                }
            }
            _ = report_tick.tick() => {
                let (peer, packet) = {
                    let mut state = state.lock().unwrap();
                    let blocks = state.report_blocks(Instant::now());
                    (state.peer, build_liveness(our_ssrc, &cname, &blocks))
                };
                send(peer, packet).await;
            }
            _ = corruption.notified() => {
                let built = {
                    let mut state = state.lock().unwrap();
                    match state.media_ssrc {
                        Some(media_ssrc) => {
                            let blocks = state.report_blocks(Instant::now());
                            fir_sequence = fir_sequence.wrapping_add(1);
                            Some((state.peer, build_keyframe_request(
                                our_ssrc,
                                &cname,
                                media_ssrc,
                                &blocks,
                                fir_sequence,
                            )))
                        }
                        None => None,
                    }
                };
                if let Some((peer, packet)) = built {
                    tracing::info!("requesting keyframe via RTCP (PLI+FIR)");
                    send(peer, packet).await;
                }
                tokio::time::sleep(KEYFRAME_DEBOUNCE).await;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reception_stats_handle_wrap_and_loss() {
        let mut stats = ReceptionStats::default();
        for sequence in [u16::MAX - 1, u16::MAX, 0, 2] {
            stats.on_packet(sequence);
        }
        let block = stats.report_block(0x1234, 0, 0);
        assert_eq!(block.highest_seq, 65_538);
        assert_eq!(block.cumulative_lost, 1);
        assert_eq!(block.fraction_lost, 51);
        assert_eq!(stats.highest_sequence_relative(), 4);
    }

    #[test]
    fn source_reset_discards_sequence_state_and_preserves_session_frame_count() {
        let mut state = RtcpShared::default();
        state.note_rtp_packet(1, 100, true);
        state.note_rtp_packet(1, 101, true);
        state.reset_media_source(2);
        state.note_rtp_packet(2, 7, true);

        assert_eq!(state.media_ssrc, Some(2));
        assert_eq!(state.frames, 3);
        assert_eq!(state.stats.highest_sequence_relative(), 0);
    }
}
