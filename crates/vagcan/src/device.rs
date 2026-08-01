//! Choosing which USB-CAN adapter to talk to.
//!
//! A person running this on a car has exactly one adapter plugged in, so
//! requiring them to paste a `/dev/cu.usbmodem…` path every time is friction
//! for nothing. `--device` stays available for the ambiguous cases; when it is
//! omitted we pick the obvious candidate and say which one we picked.

use anyhow::{bail, Result};
use vag_can::{list_adapters, AdapterInfo};

/// Resolve the adapter to open.
///
/// With `--device` given, that path is used verbatim — no guessing behind the
/// user's back. Without it: exactly one candidate is used automatically, none
/// or several is an error that lists what was found, because silently picking
/// one of two adapters is how you end up talking to the wrong bus.
pub fn resolve(requested: Option<&str>) -> Result<String> {
    if let Some(path) = requested {
        return Ok(path.to_string());
    }

    let found = list_adapters()?;

    // A recognised CAN adapter wins outright. Someone with a CANable plugged in
    // next to an Arduino means the CANable, and making them spell that out
    // every time is friction for nothing.
    let known: Vec<&AdapterInfo> = found.iter().filter(|a| a.known).collect();
    if known.len() == 1 {
        let only = known[0];
        eprintln!("using {} ({})", only.path, only.description);
        return Ok(only.path.clone());
    }

    match found.len() {
        0 => bail!(
            "no USB-CAN adapter found.\n\
             Plug one in and check it enumerated: `vagcan devices`.\n\
             If it is plugged in but missing, unplug and replug it — the adapter can \
             enumerate on USB without macOS attaching a serial node."
        ),
        1 => {
            let only = &found[0];
            eprintln!("using {} ({})", only.path, only.description);
            Ok(only.path.clone())
        }
        _ => {
            let list = found
                .iter()
                .map(|a| format!("  --device {}   {}", a.path, a.description))
                .collect::<Vec<_>>()
                .join("\n");
            bail!("several serial devices found — say which one:\n{list}")
        }
    }
}

/// Every candidate adapter, for `vagcan devices`.
pub fn list() -> Result<Vec<AdapterInfo>> {
    Ok(list_adapters()?)
}

/// Render the device list for a human.
pub fn render_list(found: &[AdapterInfo]) -> String {
    if found.is_empty() {
        return "No serial devices found.\n\n\
                If your adapter is plugged in, unplug and replug it: it can enumerate on USB \
                without macOS attaching a serial node, and then there is nothing to open."
            .to_string();
    }
    let mut out = String::from("Serial devices:\n\n");
    for a in found {
        let mark = if a.known { "*" } else { " " };
        out.push_str(&format!("{mark} {}\n    {}\n", a.path, a.description));
    }
    out.push_str("\n* = recognised CAN adapter. Pass one with --device, or omit --device when\n");
    out.push_str("  only one is connected.");
    out
}

/// What to say when the adapter will not open.
///
/// The `devices` command exists precisely for this moment and used to be
/// unreachable from it: nothing ever told the user it was there.
pub fn open_failure(path: &str) -> String {
    format!(
        "opening the adapter at {path} — run `vagcan devices` to list what is connected"
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn adapter(path: &str, desc: &str, known: bool) -> AdapterInfo {
        AdapterInfo { path: path.to_string(), description: desc.to_string(), known }
    }

    #[test]
    fn a_recognised_adapter_wins_over_anonymous_serial_devices() {
        // The situation on the real machine: a CANable plus whatever else the
        // laptop exposes. Picking must not degrade into "several found".
        let found = [
            adapter("/dev/cu.usbmodem1", "CANable 2.0 (slcan)", true),
            adapter("/dev/cu.usbserial", "USB 0403:6001", false),
        ];
        let known: Vec<&AdapterInfo> = found.iter().filter(|a| a.known).collect();
        assert_eq!(known.len(), 1);
        assert_eq!(known[0].path, "/dev/cu.usbmodem1");
    }

    #[test]
    fn an_explicit_device_is_used_verbatim() {
        // Never second-guess an explicit choice, even a path that looks odd.
        assert_eq!(resolve(Some("/dev/whatever")).unwrap(), "/dev/whatever");
    }

    #[test]
    fn the_listing_marks_recognised_adapters_and_explains_the_mark() {
        let text = render_list(&[
            adapter("/dev/cu.usbmodem1", "CANable 2.0 (slcan)", true),
            adapter("/dev/cu.usbserial", "USB 0403:6001", false),
        ]);
        assert!(text.contains("* /dev/cu.usbmodem1"), "{text}");
        assert!(text.contains("  /dev/cu.usbserial"), "{text}");
        assert!(text.contains("recognised CAN adapter"), "{text}");
    }

    #[test]
    fn an_empty_listing_explains_the_enumeration_trap() {
        // The failure that actually happened on the car: the adapter present on
        // USB, no serial node, every open failing with "No such file".
        let text = render_list(&[]);
        assert!(text.contains("unplug and replug"), "{text}");
    }
}
