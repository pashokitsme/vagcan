//! Hardware checkpoint (M4 + cable plugged). Ignored in `cargo test` — no
//! hardware in CI. Run manually:
//!
//! ```sh
//! cargo test -p vag-hex --test open_real_cable -- --ignored --nocapture
//! ```
//!
//! It enumerates the FTDI bus, opens the first HEX cable, and does one probe
//! write, exercising the whole D2XX path against the vendored dylib.

use std::time::Duration;

use vag_hex::{Backend, D2xxBackend, FTDI_VID, list_cables};

#[test]
#[ignore = "requires the physical FTDI HEX cable"]
fn open_real_cable() {
    let cables = list_cables().expect("enumerate FTDI devices");
    println!("found {} FTDI device(s):", cables.len());
    for c in &cables {
        println!(
            "  serial={:?} desc={:?} vid={:#06x} pid={:#06x}",
            c.serial, c.description, c.vid, c.pid
        );
    }
    assert!(!cables.is_empty(), "no FTDI cable enumerated");
    assert!(
        cables.iter().any(|c| c.vid == FTDI_VID),
        "no device with the FTDI vendor id {FTDI_VID:#06x}"
    );

    // Open the first device, program params, and issue one plaintext probe
    // frame (`53 04 02 55`). We drive it through the async Backend on a small
    // current-thread runtime.
    let rt = tokio::runtime::Builder::new_current_thread()
        .enable_time()
        .build()
        .unwrap();
    rt.block_on(async {
        let mut cable = D2xxBackend::open(None).expect("open first cable");
        cable
            .write(&[0x53, 0x04, 0x02, 0x55])
            .await
            .expect("write probe frame");

        let mut buf = [0u8; 64];
        let n = cable.read(&mut buf).await.expect("read reply");
        println!("read {n} byte(s): {:02x?}", &buf[..n]);
        // No assertion on content: the cable's exact reply is pinned by the
        // handshake task. Getting here without error is the checkpoint.
        let _ = Duration::from_millis(0);
    });
}
