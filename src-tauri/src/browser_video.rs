//! Browser HEVC transport shared by the device session and WebSocket clients.

use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

use bytes::Bytes;
use tokio::sync::Notify;
use tokio::sync::broadcast;

// Cover short WebSocket stalls while a large IRAP access unit is copied into the
// platform WebView. At 60 FPS this retains roughly half a second of compressed data.
const CHANNEL_CAPACITY: usize = 32;
const PACKET_MAGIC: &[u8; 4] = b"DHV1";
const PACKET_HEADER_LEN: usize = 28;

#[derive(Debug)]
pub struct BrowserVideoFrame {
    pub sequence: u64,
    pub timestamp_us: u64,
    pub key: bool,
    pub width: u16,
    pub height: u16,
    pub bytes: Bytes,
}

struct BrowserVideoSlotInner {
    sender: broadcast::Sender<Arc<BrowserVideoFrame>>,
    sequence: AtomicU64,
    dimensions: AtomicU64,
    keyframe: Notify,
}

#[derive(Clone)]
pub struct BrowserVideoSlot(Arc<BrowserVideoSlotInner>);

impl Default for BrowserVideoSlot {
    fn default() -> Self {
        let (sender, _) = broadcast::channel(CHANNEL_CAPACITY);
        Self(Arc::new(BrowserVideoSlotInner {
            sender,
            sequence: AtomicU64::new(0),
            dimensions: AtomicU64::new(0),
            keyframe: Notify::new(),
        }))
    }
}

impl BrowserVideoSlot {
    pub fn publish(&self, timestamp_us: u64, key: bool, width: u16, height: u16, bytes: Vec<u8>) {
        let sequence = self.0.sequence.fetch_add(1, Ordering::Relaxed) + 1;
        self.0.dimensions.store(
            (u64::from(width) << 32) | u64::from(height),
            Ordering::Relaxed,
        );
        let _ = self.0.sender.send(Arc::new(BrowserVideoFrame {
            sequence,
            timestamp_us,
            key,
            width,
            height,
            bytes: Bytes::from(bytes),
        }));
    }

    pub fn subscribe(&self) -> broadcast::Receiver<Arc<BrowserVideoFrame>> {
        self.0.sender.subscribe()
    }

    pub fn version(&self) -> u64 {
        self.0.sequence.load(Ordering::Relaxed)
    }

    pub fn dimensions(&self) -> Option<(u32, u32)> {
        let packed = self.0.dimensions.load(Ordering::Relaxed);
        let width = (packed >> 32) as u32;
        let height = packed as u32;
        (width > 0 && height > 0).then_some((width, height))
    }

    pub fn reset_dimensions(&self) {
        self.0.dimensions.store(0, Ordering::Relaxed);
    }

    pub fn request_keyframe(&self) {
        self.0.keyframe.notify_one();
    }

    pub async fn keyframe_requested(&self) {
        self.0.keyframe.notified().await;
    }
}

/// Encode a versioned message without copying the access-unit payload twice.
pub fn encode_packet(frame: &BrowserVideoFrame) -> Vec<u8> {
    let mut packet = Vec::with_capacity(PACKET_HEADER_LEN + frame.bytes.len());
    packet.extend_from_slice(PACKET_MAGIC);
    packet.push(u8::from(frame.key));
    packet.extend_from_slice(&[0, 0, 0]);
    packet.extend_from_slice(&frame.timestamp_us.to_be_bytes());
    packet.extend_from_slice(&frame.sequence.to_be_bytes());
    packet.extend_from_slice(&frame.width.to_be_bytes());
    packet.extend_from_slice(&frame.height.to_be_bytes());
    packet.extend_from_slice(&frame.bytes);
    packet
}

pub fn hevc_dimensions(access_unit: &[u8]) -> Option<(u16, u16)> {
    annexb_nals(access_unit).find_map(|nal| {
        (nal.len() >= 2 && ((nal[0] >> 1) & 0x3f) == 33)
            .then(|| parse_hevc_sps_dimensions(nal))
            .flatten()
            .and_then(|(width, height)| {
                Some((u16::try_from(width).ok()?, u16::try_from(height).ok()?))
            })
    })
}

fn annexb_nals(bytes: &[u8]) -> impl Iterator<Item = &[u8]> {
    let mut ranges = Vec::new();
    let mut index = 0;
    let mut current = None;
    while index + 3 <= bytes.len() {
        let start_len = if index + 4 <= bytes.len() && bytes[index..].starts_with(&[0, 0, 0, 1]) {
            4
        } else if bytes[index..].starts_with(&[0, 0, 1]) {
            3
        } else {
            index += 1;
            continue;
        };
        if let Some(start) = current {
            ranges.push(start..index);
        }
        current = Some(index + start_len);
        index += start_len;
    }
    if let Some(start) = current
        && start < bytes.len()
    {
        ranges.push(start..bytes.len());
    }
    ranges.into_iter().map(|range| &bytes[range])
}

fn parse_hevc_sps_dimensions(nal: &[u8]) -> Option<(u32, u32)> {
    let rbsp = ebsp_to_rbsp(nal.get(2..)?);
    let mut reader = BitReader::new(&rbsp);
    reader.read_bits(4)?;
    let max_sub_layers_minus1 = reader.read_bits(3)? as usize;
    reader.read_bit()?;
    skip_profile_tier_level(&mut reader, max_sub_layers_minus1)?;
    reader.read_ue()?;
    let chroma_format_idc = reader.read_ue()?;
    if chroma_format_idc == 3 {
        reader.read_bit()?;
    }
    let picture_width = reader.read_ue()?;
    let picture_height = reader.read_ue()?;
    let mut left = 0;
    let mut right = 0;
    let mut top = 0;
    let mut bottom = 0;
    if reader.read_bit()? {
        left = reader.read_ue()?;
        right = reader.read_ue()?;
        top = reader.read_ue()?;
        bottom = reader.read_ue()?;
    }
    let (sub_width, sub_height) = match chroma_format_idc {
        0 | 3 => (1, 1),
        1 => (2, 2),
        2 => (2, 1),
        _ => return None,
    };
    let width = picture_width.saturating_sub((left + right) * sub_width);
    let height = picture_height.saturating_sub((top + bottom) * sub_height);
    (width > 0 && height > 0).then_some((width, height))
}

fn skip_profile_tier_level(reader: &mut BitReader<'_>, layers: usize) -> Option<()> {
    reader.read_bits(2)?;
    reader.read_bit()?;
    reader.read_bits(5)?;
    reader.read_bits(32)?;
    reader.read_bits(48)?;
    reader.read_bits(8)?;
    let mut profile_present = [false; 8];
    let mut level_present = [false; 8];
    for index in 0..layers {
        profile_present[index] = reader.read_bit()?;
        level_present[index] = reader.read_bit()?;
    }
    if layers > 0 {
        for _ in layers..8 {
            reader.read_bits(2)?;
        }
    }
    for index in 0..layers {
        if profile_present[index] {
            reader.read_bits(2)?;
            reader.read_bit()?;
            reader.read_bits(5)?;
            reader.read_bits(32)?;
            reader.read_bits(48)?;
        }
        if level_present[index] {
            reader.read_bits(8)?;
        }
    }
    Some(())
}

fn ebsp_to_rbsp(bytes: &[u8]) -> Vec<u8> {
    let mut output = Vec::with_capacity(bytes.len());
    let mut zeros = 0;
    for &byte in bytes {
        if zeros == 2 && byte == 0x03 {
            zeros = 0;
            continue;
        }
        output.push(byte);
        zeros = if byte == 0 { zeros + 1 } else { 0 };
    }
    output
}

struct BitReader<'a> {
    bytes: &'a [u8],
    bit: usize,
}

impl<'a> BitReader<'a> {
    fn new(bytes: &'a [u8]) -> Self {
        Self { bytes, bit: 0 }
    }

    fn read_bit(&mut self) -> Option<bool> {
        let byte = *self.bytes.get(self.bit / 8)?;
        let value = (byte >> (7 - self.bit % 8)) & 1;
        self.bit += 1;
        Some(value != 0)
    }

    fn read_bits(&mut self, count: usize) -> Option<u64> {
        let mut value = 0;
        for _ in 0..count {
            value = (value << 1) | u64::from(self.read_bit()?);
        }
        Some(value)
    }

    fn read_ue(&mut self) -> Option<u32> {
        let mut leading_zeros = 0;
        while !self.read_bit()? {
            leading_zeros += 1;
            if leading_zeros > 31 {
                return None;
            }
        }
        let suffix = self.read_bits(leading_zeros)? as u32;
        Some((1_u32 << leading_zeros) - 1 + suffix)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn browser_packet_has_stable_big_endian_header() {
        let frame = BrowserVideoFrame {
            sequence: 7,
            timestamp_us: 16_667,
            key: true,
            width: 1290,
            height: 2796,
            bytes: Bytes::from_static(&[0, 0, 0, 1, 0x26]),
        };
        let packet = encode_packet(&frame);
        assert_eq!(&packet[..4], PACKET_MAGIC);
        assert_eq!(packet[4], 1);
        assert_eq!(
            u64::from_be_bytes(packet[8..16].try_into().unwrap()),
            16_667
        );
        assert_eq!(u64::from_be_bytes(packet[16..24].try_into().unwrap()), 7);
        assert_eq!(u16::from_be_bytes(packet[24..26].try_into().unwrap()), 1290);
        assert_eq!(u16::from_be_bytes(packet[26..28].try_into().unwrap()), 2796);
        assert_eq!(&packet[28..], frame.bytes.as_ref());
    }

    #[tokio::test]
    async fn browser_channel_absorbs_a_short_websocket_stall() {
        let slot = BrowserVideoSlot::default();
        let mut receiver = slot.subscribe();
        for timestamp in 0..CHANNEL_CAPACITY {
            slot.publish(
                timestamp as u64,
                timestamp == 0,
                100,
                200,
                vec![timestamp as u8],
            );
        }

        for sequence in 1..=CHANNEL_CAPACITY {
            let frame = receiver.recv().await.expect("buffered browser frame");
            assert_eq!(frame.sequence, sequence as u64);
        }
    }

    #[test]
    fn annexb_iterator_skips_start_codes() {
        let bytes = [0, 0, 0, 1, 0x40, 1, 0, 0, 1, 0x42, 1];
        assert_eq!(
            annexb_nals(&bytes).collect::<Vec<_>>(),
            vec![&bytes[4..6], &bytes[9..]]
        );
    }

    #[test]
    fn malformed_sps_is_rejected_without_panicking() {
        for bytes in [
            &[][..],
            &[0, 0, 1, 0x42][..],
            &[0, 0, 0, 1, 0x42, 0xff][..],
            &[0, 0, 0, 1, 0x42, 0x01, 0, 0, 0, 0][..],
        ] {
            assert_eq!(hevc_dimensions(bytes), None);
        }
    }
}
