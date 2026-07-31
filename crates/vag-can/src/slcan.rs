//! slcan (LAWICEL) ASCII framing over a serial byte stream.
//!
//! The codec and the stream-generic [`SlcanBackend`] always build; only the
//! real-serial-port constructor needs the `slcan` feature (tokio-serial).

use std::time::Duration;
use tokio::io::{AsyncRead, AsyncReadExt as _, AsyncWrite, AsyncWriteExt as _};
use tokio::time::Instant;

use crate::CanError;
use crate::backend::{CAN_EFF_FLAG, CAN_EFF_MASK, CAN_SFF_MASK, CanBackend};

/// CAN bitrate presets (`Sn` slcan setup command). VAG diagnostic CAN is 500k.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SlcanBitrate {
    Rate10k = 0,
    Rate20k = 1,
    Rate50k = 2,
    Rate100k = 3,
    Rate125k = 4,
    Rate250k = 5,
    Rate500k = 6,
    Rate800k = 7,
    Rate1m = 8,
}

/// How the adapter drives the bus once the channel opens (`Mn` command).
///
/// [`Silent`](SlcanMode::Silent) is listen-only: the controller receives but
/// never writes a bit — not even an acknowledge — so it cannot disturb a bus
/// another tester is using. That is the mode for sniffing a live VCDS session.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum SlcanMode {
    /// Normal: receives, acknowledges, and may transmit.
    #[default]
    Normal = 0,
    /// Listen-only ("silent"): receives only, never drives the bus.
    Silent = 1,
}

/// Encode one classic CAN frame as an slcan ASCII line (with trailing CR).
pub fn encode_frame(id: u32, data: &[u8]) -> Result<String, CanError> {
    if data.len() > 8 {
        return Err(CanError::Unsupported("classic CAN frame data must be <= 8 bytes"));
    }
    let mut line = if id & CAN_EFF_FLAG != 0 {
        format!("T{:08X}{:X}", id & CAN_EFF_MASK, data.len())
    } else {
        format!("t{:03X}{:X}", id & CAN_SFF_MASK, data.len())
    };
    for b in data {
        line.push_str(&format!("{b:02X}"));
    }
    line.push('\r');
    Ok(line)
}

fn hex_field(s: &str, range: std::ops::Range<usize>) -> Result<u32, CanError> {
    let field = s
        .get(range)
        .ok_or_else(|| CanError::MalformedFrame(format!("slcan line too short: {s:?}")))?;
    u32::from_str_radix(field, 16)
        .map_err(|_| CanError::MalformedFrame(format!("bad hex {field:?} in {s:?}")))
}

/// Decode one slcan ASCII line (`t...`/`T...`) into `(raw_id, data)`.
///
/// Trailing bytes after the data field (e.g. adapter timestamps) are ignored.
pub fn decode_frame(line: &str) -> Result<(u32, Vec<u8>), CanError> {
    let line = line.trim_end_matches(['\r', '\n']);
    let (id, dlc_pos) = match line.as_bytes().first() {
        Some(b't') => (hex_field(line, 1..4)?, 4),
        Some(b'T') => (hex_field(line, 1..9)? | CAN_EFF_FLAG, 9),
        _ => return Err(CanError::MalformedFrame(format!("not an slcan frame: {line:?}"))),
    };
    let dlc = hex_field(line, dlc_pos..dlc_pos + 1)? as usize;
    if dlc > 8 {
        return Err(CanError::MalformedFrame(format!("dlc {dlc} > 8 in {line:?}")));
    }
    let mut data = Vec::with_capacity(dlc);
    for i in 0..dlc {
        let at = dlc_pos + 1 + i * 2;
        data.push(hex_field(line, at..at + 2)? as u8);
    }
    Ok((id, data))
}

/// slcan backend over any async byte stream (serial port, `tokio::io::duplex`
/// in tests). Skips non-frame lines (command acks, status) on receive.
pub struct SlcanBackend<S> {
    stream: S,
    buf: Vec<u8>,
}

impl<S: AsyncRead + AsyncWrite + Unpin + Send> SlcanBackend<S> {
    pub fn new(stream: S) -> Self {
        SlcanBackend { stream, buf: Vec::new() }
    }

    /// Send the channel-open sequence in [`SlcanMode::Normal`] — see
    /// [`Self::open_channel_mode`].
    pub async fn open_channel(&mut self, bitrate: SlcanBitrate) -> Result<(), CanError> {
        self.open_channel_mode(bitrate, SlcanMode::Normal).await
    }

    /// Send the channel-open sequence: close, set bitrate, set the bus mode,
    /// open. Fire-and-forget: many adapters NAK a redundant `C` (and the
    /// CANable2 firmware acks nothing at all), so acks are not checked.
    ///
    /// **The mode command must precede `O`.** The controller only accepts mode
    /// configuration while it is in init state; after the channel opens the
    /// registers are locked and a later `M` is silently ignored.
    ///
    /// The mode is sent explicitly on *every* open, including
    /// [`SlcanMode::Normal`]. The adapter keeps its silent flag across a
    /// close/open cycle, so a normal open that omitted `M0` would inherit
    /// listen-only from an earlier sniffing run and transmit nothing.
    pub async fn open_channel_mode(
        &mut self,
        bitrate: SlcanBitrate,
        mode: SlcanMode,
    ) -> Result<(), CanError> {
        let cmd = format!("C\rS{}\rM{}\rO\r", bitrate as u8, mode as u8);
        self.write_all(cmd.as_bytes()).await
    }

    /// Send the channel-close command.
    pub async fn close_channel(&mut self) -> Result<(), CanError> {
        self.write_all(b"C\r").await
    }

    async fn write_all(&mut self, bytes: &[u8]) -> Result<(), CanError> {
        self.stream
            .write_all(bytes)
            .await
            .map_err(|e| CanError::Io(e.to_string()))?;
        self.stream.flush().await.map_err(|e| CanError::Io(e.to_string()))
    }

    /// Next CR-terminated line (without the CR), reading more bytes as needed.
    async fn read_line(&mut self, deadline: Instant) -> Result<Vec<u8>, CanError> {
        loop {
            if let Some(pos) = self.buf.iter().position(|&b| b == b'\r') {
                let mut line: Vec<u8> = self.buf.drain(..=pos).collect();
                line.pop(); // drop the CR
                return Ok(line);
            }
            let remaining = deadline.saturating_duration_since(Instant::now());
            if remaining.is_zero() {
                return Err(CanError::Timeout);
            }
            let mut chunk = [0u8; 256];
            let n = tokio::time::timeout(remaining, self.stream.read(&mut chunk))
                .await
                .map_err(|_| CanError::Timeout)?
                .map_err(|e| CanError::Io(e.to_string()))?;
            if n == 0 {
                return Err(CanError::Disconnected);
            }
            self.buf.extend_from_slice(&chunk[..n]);
        }
    }
}

impl<S: AsyncRead + AsyncWrite + Unpin + Send> CanBackend for SlcanBackend<S> {
    async fn send_frame(&mut self, id: u32, data: &[u8]) -> Result<(), CanError> {
        let line = encode_frame(id, data)?;
        self.write_all(line.as_bytes()).await
    }

    async fn recv_frame(&mut self, timeout: Duration) -> Result<(u32, Vec<u8>), CanError> {
        let deadline = Instant::now() + timeout;
        loop {
            let line = self.read_line(deadline).await?;
            // Strip stray BEL (error ack) bytes; they are not CR-terminated.
            let text = String::from_utf8_lossy(&line);
            let text = text.trim_matches(|c: char| c == '\u{7}' || c.is_whitespace());
            match text.as_bytes().first() {
                Some(b't' | b'T') => return decode_frame(text),
                // Command acks ('z', 'Z', version/status replies) and empty
                // lines are not bus traffic — skip them.
                _ => continue,
            }
        }
    }
}

/// Open a real slcan serial adapter and open its CAN channel.
#[cfg(feature = "slcan")]
impl SlcanBackend<tokio_serial::SerialStream> {
    pub async fn open(path: &str, baud: u32, bitrate: SlcanBitrate) -> Result<Self, CanError> {
        SlcanBackend::open_mode(path, baud, bitrate, SlcanMode::Normal).await
    }

    /// Open a real slcan serial adapter and open its CAN channel in `mode`.
    /// [`SlcanMode::Silent`] is the safe way onto a bus somebody else is using.
    pub async fn open_mode(
        path: &str,
        baud: u32,
        bitrate: SlcanBitrate,
        mode: SlcanMode,
    ) -> Result<Self, CanError> {
        use tokio_serial::SerialPortBuilderExt;
        let stream = tokio_serial::new(path, baud)
            .open_native_async()
            .map_err(|e| CanError::Io(e.to_string()))?;
        let mut backend = SlcanBackend::new(stream);
        backend.open_channel_mode(bitrate, mode).await?;
        Ok(backend)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::CAN_EFF_FLAG;

    #[test]
    fn encodes_standard_frame() {
        let line = encode_frame(0x7E0, &[0x02, 0x10, 0x03, 0, 0, 0, 0, 0]).unwrap();
        assert_eq!(line, "t7E080210030000000000\r");
    }

    #[test]
    fn encodes_extended_frame() {
        let line = encode_frame(0x18DA_10F1 | CAN_EFF_FLAG, &[0x3E, 0x00]).unwrap();
        assert_eq!(line, "T18DA10F123E00\r");
    }

    #[test]
    fn encode_rejects_oversized_data() {
        let err = encode_frame(0x7E0, &[0u8; 9]).unwrap_err();
        assert!(matches!(err, CanError::Unsupported(_)), "got {err:?}");
    }

    #[test]
    fn decodes_standard_frame() {
        let (id, data) = decode_frame("t7E88025003AAAAAAAAAA\r").unwrap();
        assert_eq!(id, 0x7E8);
        assert_eq!(data, vec![0x02, 0x50, 0x03, 0xAA, 0xAA, 0xAA, 0xAA, 0xAA]);
    }

    #[test]
    fn decodes_extended_frame() {
        let (id, data) = decode_frame("T18DAF11027E00").unwrap();
        assert_eq!(id, 0x18DA_F110 | CAN_EFF_FLAG);
        assert_eq!(data, vec![0x7E, 0x00]);
    }

    #[test]
    fn decode_tolerates_trailing_timestamp() {
        // CANtact-style adapters append a 4-hex-digit timestamp.
        let (id, data) = decode_frame("t7E82500312AB").unwrap();
        assert_eq!(id, 0x7E8);
        assert_eq!(data, vec![0x50, 0x03]);
    }

    #[test]
    fn decode_rejects_garbage() {
        for bad in ["", "x123", "t7E", "t7E09", "t7E01ZZ"] {
            assert!(decode_frame(bad).is_err(), "accepted {bad:?}");
        }
    }

    #[tokio::test]
    async fn backend_writes_frame_as_ascii_line() {
        let (client, mut adapter) = tokio::io::duplex(256);
        let mut backend = SlcanBackend::new(client);
        backend.send_frame(0x7E0, &[0x02, 0x10, 0x03]).await.unwrap();

        let mut got = vec![0u8; 64];
        let n = adapter.read(&mut got).await.unwrap();
        assert_eq!(&got[..n], b"t7E03021003\r");
    }

    #[tokio::test]
    async fn backend_parses_incoming_frame_and_skips_acks() {
        let (client, mut adapter) = tokio::io::duplex(256);
        let mut backend = SlcanBackend::new(client);
        // tx-ack 'z', a bare CR, a BEL error byte, then the actual frame.
        adapter.write_all(b"z\r\r\x07t7E825003\r").await.unwrap();

        let (id, data) = backend.recv_frame(Duration::from_millis(200)).await.unwrap();
        assert_eq!(id, 0x7E8);
        assert_eq!(data, vec![0x50, 0x03]);
    }

    #[tokio::test]
    async fn backend_recv_times_out() {
        let (client, _adapter) = tokio::io::duplex(256);
        let mut backend = SlcanBackend::new(client);
        let err = backend.recv_frame(Duration::from_millis(10)).await.unwrap_err();
        assert!(matches!(err, CanError::Timeout), "got {err:?}");
    }

    #[tokio::test]
    async fn backend_recv_reports_disconnect() {
        let (client, adapter) = tokio::io::duplex(256);
        drop(adapter);
        let mut backend = SlcanBackend::new(client);
        let err = backend.recv_frame(Duration::from_millis(50)).await.unwrap_err();
        assert!(matches!(err, CanError::Disconnected), "got {err:?}");
    }

    #[tokio::test]
    async fn open_channel_sends_close_bitrate_mode_open() {
        // The default open is explicitly NORMAL mode: `M0` must be sent, or a
        // channel left silent by an earlier sniff would stay listen-only.
        let (client, mut adapter) = tokio::io::duplex(256);
        let mut backend = SlcanBackend::new(client);
        backend.open_channel(SlcanBitrate::Rate500k).await.unwrap();

        let mut got = vec![0u8; 64];
        let n = adapter.read(&mut got).await.unwrap();
        assert_eq!(&got[..n], b"C\rS6\rM0\rO\r");
    }

    #[tokio::test]
    async fn silent_mode_is_requested_before_the_channel_opens() {
        // The controller latches its bus mode at open: `M1` after `O` would be
        // ignored and we would ACK on a bus we promised only to listen to.
        let (client, mut adapter) = tokio::io::duplex(256);
        let mut backend = SlcanBackend::new(client);
        backend
            .open_channel_mode(SlcanBitrate::Rate500k, SlcanMode::Silent)
            .await
            .unwrap();

        let mut got = vec![0u8; 64];
        let n = adapter.read(&mut got).await.unwrap();
        assert_eq!(&got[..n], b"C\rS6\rM1\rO\r");

        let text = std::str::from_utf8(&got[..n]).unwrap();
        assert!(
            text.find("M1").unwrap() < text.find('O').unwrap(),
            "mode must be set while the controller is still in init state: {text:?}"
        );
    }
}
