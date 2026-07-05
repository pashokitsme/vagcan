//! Open-time PLAINTEXT handshake: the fixed exchange VCDS does right after
//! opening the cable, before any car traffic. This drives only the plaintext
//! open sequence and reads the cable's identity.
//!
//! Ground truth: `research/vag-hex-framing.md` §1 + §4. The plaintext open
//! sequence rides the flat `S/M` frame:
//! - `0x02` probe / ping — OUT `02` → IN `02 016044…`,
//! - `0x04` identify / version — OUT `04` → IN `04 "ROSSTECH" 000000 <ver> …`.
//!
//! **Scope boundary (see `research/SCOPE-BOUNDARY.md`).** This is Surface 3
//! (plaintext identify) only. The `0xb0..0xb5` setup burst, the `0xb6`
//! anti-clone AUTH, and the encrypted `0xb8`/`0xb7` diagnostic session are all
//! OUT OF SCOPE here — [`handshake`] stops at plaintext identify and never
//! drives auth. The `0x82`/`0x0d` status reads that follow identify are
//! optional and not needed for the identity, so they are not driven.

use std::time::Duration;

use crate::actor::CableHandle;
use crate::error::HexError;
use crate::frame::{OP_IDENTIFY, OP_PROBE};

/// Per-request timeout for the plaintext handshake frames. The cable answers the
/// open sequence promptly; this is generous headroom, not a tuned value.
pub const HANDSHAKE_TIMEOUT: Duration = Duration::from_millis(1_000);

/// Identity recovered from the cable during the plaintext handshake — surfaced
/// by `vagcan doctor`.
#[derive(Debug, Clone, Default)]
pub struct CableIdentity {
    /// Firmware / version string extracted from the identify reply (the ASCII
    /// vendor tag, e.g. `"ROSSTECH"`, plus the trailing version bytes in hex).
    /// `None` only if the reply carried no printable identity.
    pub firmware: Option<String>,
    /// Raw identify payload bytes as returned (after the `0x04` opcode, before
    /// the frame checksum), for diagnostics.
    pub raw: Vec<u8>,
}

/// Run the PLAINTEXT open handshake and return the cable's [`CableIdentity`].
///
/// Sends `0x02` (probe) then `0x04` (identify) over the connection actor via
/// [`CableHandle::request`], validates each reply's echoed opcode, and parses
/// the identify reply into the identity ("ROSSTECH" + version bytes).
///
/// **Stops at plaintext identify.** It does NOT drive the `0xb0..0xb5` setup
/// burst, the `0xb6` anti-clone auth, or the encrypted diagnostic session — see
/// the module docs and `research/SCOPE-BOUNDARY.md`.
///
/// Returns [`HexError::Handshake`] if the cable answers with the wrong opcode or
/// an identify reply that carries no printable identity, and propagates
/// [`HexError::Timeout`]/[`HexError::Io`] from the underlying transport.
pub async fn handshake(cable: &CableHandle) -> Result<CableIdentity, HexError> {
    // 1. Probe / ping. We only assert the cable echoes the probe opcode; the
    //    `01 60 44`-style payload carries no identity we depend on.
    let probe = cable.request(OP_PROBE, &[], HANDSHAKE_TIMEOUT).await?;
    if probe.opcode != OP_PROBE {
        return Err(HexError::Handshake(format!(
            "probe: cable echoed opcode {:#04x}, expected {OP_PROBE:#04x}",
            probe.opcode
        )));
    }

    // 2. Identify / version → the ASCII "ROSSTECH" tag + version bytes.
    let ident = cable.request(OP_IDENTIFY, &[], HANDSHAKE_TIMEOUT).await?;
    if ident.opcode != OP_IDENTIFY {
        return Err(HexError::Handshake(format!(
            "identify: cable echoed opcode {:#04x}, expected {OP_IDENTIFY:#04x}",
            ident.opcode
        )));
    }

    // Plaintext PoC stops here — do NOT proceed into b0..b6 / encrypted session.
    parse_identify(&ident.data)
}

/// Parse an identify reply payload into a [`CableIdentity`].
///
/// Layout (from the capture): a leading printable-ASCII vendor tag
/// (`"ROSSTECH"`), NUL padding, then version bytes — e.g.
/// `52 4f 53 53 54 45 43 48  00 00 00  a8 9d 01 00 09`. The firmware string is
/// the ASCII tag plus the version bytes rendered as hex. A reply with no leading
/// printable ASCII is rejected as a handshake mismatch.
fn parse_identify(data: &[u8]) -> Result<CableIdentity, HexError> {
    let ascii_len = data.iter().take_while(|b| b.is_ascii_graphic()).count();
    if ascii_len == 0 {
        return Err(HexError::Handshake(format!(
            "identify: no ASCII identity in reply {data:02x?}"
        )));
    }
    let ascii = String::from_utf8_lossy(&data[..ascii_len]).into_owned();

    // Version bytes follow the ASCII tag, past its NUL padding.
    let version: Vec<u8> = data[ascii_len..]
        .iter()
        .copied()
        .skip_while(|&b| b == 0)
        .collect();

    let firmware = if version.is_empty() {
        ascii
    } else {
        let ver_hex: String = version.iter().map(|b| format!("{b:02x}")).collect();
        format!("{ascii} {ver_hex}")
    };

    Ok(CableIdentity {
        firmware: Some(firmware),
        raw: data.to_vec(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::actor::spawn;
    use crate::frame::{MARKER_CABLE, MARKER_HOST, frame_encode};
    use crate::usb::Backend;
    use std::collections::VecDeque;

    /// The real captured identify reply payload (bytes after the `0x04` opcode):
    /// `04 52 4f 53 53 54 45 43 48 00 00 00 a8 9d 01 00 09` → "ROSSTECH" + ver.
    const IDENTIFY_DATA: [u8; 16] = [
        0x52, 0x4f, 0x53, 0x53, 0x54, 0x45, 0x43, 0x48, // "ROSSTECH"
        0x00, 0x00, 0x00, // NUL padding
        0xa8, 0x9d, 0x01, 0x00, 0x09, // version bytes
    ];

    /// Scripted in-memory backend: a write that matches a scripted request frame
    /// enqueues its reply bytes onto the read stream; reads pend (cancel-safely)
    /// while the stream is empty. Mirrors the actor-test mock.
    struct ScriptedBackend {
        /// `(expected host wire frame, reply wire bytes to enqueue)`.
        replies: Vec<(Vec<u8>, Vec<u8>)>,
        inbox: VecDeque<u8>,
    }

    impl Backend for ScriptedBackend {
        async fn write(&mut self, bytes: &[u8]) -> Result<(), HexError> {
            if let Some(pos) = self.replies.iter().position(|(req, _)| req == bytes) {
                let (_, reply) = self.replies.remove(pos);
                self.inbox.extend(reply);
            }
            Ok(())
        }

        async fn read(&mut self, buf: &mut [u8]) -> Result<usize, HexError> {
            if self.inbox.is_empty() {
                std::future::pending::<()>().await;
            }
            let n = buf.len().min(self.inbox.len());
            for slot in buf.iter_mut().take(n) {
                *slot = self.inbox.pop_front().unwrap();
            }
            Ok(n)
        }
    }

    fn backend(replies: Vec<(Vec<u8>, Vec<u8>)>) -> ScriptedBackend {
        ScriptedBackend {
            replies,
            inbox: VecDeque::new(),
        }
    }

    /// Probe OUT `53 04 02 55` → IN `4d 07 02 01 60 44 <xor>` (real shape).
    fn probe_script() -> (Vec<u8>, Vec<u8>) {
        (
            frame_encode(MARKER_HOST, OP_PROBE, &[]),
            frame_encode(MARKER_CABLE, OP_PROBE, &[0x01, 0x60, 0x44]),
        )
    }

    /// Identify OUT `53 04 04 53` → IN with the real captured payload.
    fn identify_script() -> (Vec<u8>, Vec<u8>) {
        (
            frame_encode(MARKER_HOST, OP_IDENTIFY, &[]),
            frame_encode(MARKER_CABLE, OP_IDENTIFY, &IDENTIFY_DATA),
        )
    }

    #[tokio::test]
    async fn handshake_returns_rosstech_identity() {
        let handle = spawn(backend(vec![probe_script(), identify_script()]));

        let id = handshake(&handle).await.expect("handshake succeeds");

        let fw = id.firmware.expect("firmware present");
        assert!(fw.contains("ROSSTECH"), "firmware = {fw:?}");
        assert!(
            fw.contains("a89d010009"),
            "version bytes parsed into firmware = {fw:?}"
        );
        assert_eq!(id.raw, IDENTIFY_DATA.to_vec());
    }

    #[tokio::test]
    async fn handshake_rejects_identify_without_ascii_identity() {
        // Identify answers with no printable identity (all NUL) -> Handshake.
        let bad_identify = (
            frame_encode(MARKER_HOST, OP_IDENTIFY, &[]),
            frame_encode(MARKER_CABLE, OP_IDENTIFY, &[0x00, 0x00, 0x00]),
        );
        let handle = spawn(backend(vec![probe_script(), bad_identify]));

        let err = handshake(&handle).await.unwrap_err();
        assert!(matches!(err, HexError::Handshake(_)), "got {err:?}");
    }

    #[tokio::test]
    async fn handshake_rejects_wrong_probe_opcode() {
        // Probe answered by a different opcode (copyright banner 0x63) -> Handshake.
        let bad_probe = (
            frame_encode(MARKER_HOST, OP_PROBE, &[]),
            frame_encode(MARKER_CABLE, 0x63, &[0xAA]),
        );
        let handle = spawn(backend(vec![bad_probe]));

        let err = handshake(&handle).await.unwrap_err();
        assert!(matches!(err, HexError::Handshake(_)), "got {err:?}");
    }

    #[tokio::test(start_paused = true)]
    async fn handshake_times_out_on_silent_cable() {
        // No scripted replies: the probe request never gets an answer.
        let handle = spawn(backend(vec![]));

        let err = handshake(&handle).await.unwrap_err();
        assert!(matches!(err, HexError::Timeout), "got {err:?}");
    }

    #[test]
    fn parse_identify_extracts_ascii_and_version() {
        let id = parse_identify(&IDENTIFY_DATA).unwrap();
        let fw = id.firmware.unwrap();
        assert!(fw.starts_with("ROSSTECH"), "firmware = {fw:?}");
        assert!(fw.contains("a89d010009"), "firmware = {fw:?}");
        assert_eq!(id.raw, IDENTIFY_DATA.to_vec());
    }

    #[test]
    fn parse_identify_keeps_ascii_only_when_no_version_bytes() {
        let id = parse_identify(b"ROSSTECH").unwrap();
        assert_eq!(id.firmware.as_deref(), Some("ROSSTECH"));
    }

    #[test]
    fn parse_identify_rejects_no_ascii() {
        assert!(matches!(
            parse_identify(&[0x00, 0x00, 0x00]),
            Err(HexError::Handshake(_))
        ));
        assert!(matches!(parse_identify(&[]), Err(HexError::Handshake(_))));
    }
}
