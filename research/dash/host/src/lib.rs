//! The bench rig's own half: the frame format the board streams over USB.
//!
//! The BLE client it used to carry moved to `vag-dash-ble`, because the product
//! needs it too and a bench rig is the wrong place for a shared library.

pub mod frame;
