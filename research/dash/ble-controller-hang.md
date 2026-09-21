> Research note, 2026-09-22 (a subagent's report, kept verbatim). Result of its first experiment: §9.13 of `can-bring-up.md` — the two IDF steps did not help, cold or warm.
# BLE controller hangs on the new ESP32-C3 (v0.4), Wi-Fi works

Research date 2026-09-22. Nothing was flashed. No serial port was opened. The repo was not edited.
Registry root below is `R = ~/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f`.
The scratch folder below is gone; the probe patch it names is kept beside this file as `ble-controller-hang.fix.diff`. Scratch work was in `S = /private/tmp/claude-501/-Users-pavel-smirnov-Source-repos-vcds/7e784b57-985b-4f07-bffe-99e9fe41165a/scratchpad`.

## TL;DR

- **Where it hangs.** `BleConnector::new` → `ble_init` → `btdm_controller_enable(BLE)` (`R/esp-wifi-0.15.1/src/ble/btdm.rs:451`). The blob posts message 9 to the controller task, then blocks forever on the ROM semaphore **`g_rw_init_sem`** (sources: disassembly and the ROM linker map, §2). The **BT controller task** (`g_rw_controller_task_handle`) is what gives it, once it has finished bringing up the controller. Nothing gives it, so that task never finishes enabling the controller.
- **The sampled PC (`semphr_take`) is the waiter, not the culprit.** `sem_take` is a yield loop (`R/esp-wifi-0.15.1/src/compat/common.rs:207-247`), so the main task always shows up there. What matters is where the *controller task* is at that moment.
- **No known public issue matches** "C3, BLE hangs, Wi-Fi fine, chip rev v0.4". The nearest one is esp-hal#6335: the same symptom, hung in `btdm_controller_enable`, but on an ESP32-S3. It was closed with no fix as "too old a version" (https://github.com/esp-rs/esp-hal/issues/6335).
- **The most likely cause is an inference**, supported by code diffs. esp-wifi 0.15.1 leaves out two steps that ESP-IDF runs before `btdm_controller_init` on the C3:
  1. `periph_module_reset(PERIPH_BT_MODULE)`. esp-wifi's reset pulse misses the **BT low-power block** bits `RW_BTLP_RST` (bit 10) and `RW_BTLP_REG_RST` (bit 12).
  2. Selecting the BT low-power clock (`btdm_lpclk_select_src(XTAL)` + `btdm_lpclk_set_div(40)`). esp-wifi leaves the reset default in place: the 8 MHz RC_FAST clock is the source, and that clock's frequency varies from chip to chip.

  ESP-IDF v4.3.4 lists a C3 fix that describes this symptom exactly: "task watchdog issue during controller initialization due to invalid hardware state after system reset on ESP32-C3". **esp-radio master (2026-09-21) still skips both steps**, so upgrading is not a known fix.
- **First experiment.** Flash the ready-built probe variant `S/probe-fix/target-fix/.../ble-scan`. It dumps registers and the reset reason, then applies both IDF steps before `BleConnector::new`. Run it once after a soft reset and once after unplugging the board (§4).

## 1. Known issues searched

| Source | Match | Result |
|---|---|---|
| esp-hal#6335 https://github.com/esp-rs/esp-hal/issues/6335 | Same stack (esp-hal rc.0 + esp-wifi 0.15.1). `BleConnector::new` hangs in `btdm_controller_enable`. JTAG shows the scheduler alive, switching between `timer_task` and `btdm_controller_task`, and the controller task never completes. **ESP32-S3** v0.2. | Closed 2026-09-15, not fixed ("12-18 month old version"). |
| esp-wifi-sys#409 https://github.com/esp-rs/esp-wifi-sys/issues/409 | S3 BLE hang in `register_chipv7_phy`. The PHY enable turned off the USB-Serial-JTAG PHY. | Fixed by `phy-enable-usb`. On 0.15.1 this is the config `phy_enable_usb`, **default true** (`R/esp-wifi-0.15.1/esp_config.yml:209-214`), so it does not apply here. |
| esp-hal#1626 https://github.com/esp-rs/esp-hal/issues/1626 | S3 BLE and Wi-Fi examples dead on one board while a sibling board with the same chip batch works. Power supply suspected. | Closed with no root cause. |
| esp-hal#3143 https://github.com/esp-rs/esp-hal/issues/3143 | Wi-Fi/BLE clock init depended on what the 2nd-stage bootloader had done. | C2, C6 and H2 affected. The issue says C3 "still works fine". Fixed in PR #3150. Not this bug. |
| esp-hal#4950 https://github.com/esp-rs/esp-hal/issues/4950 and #4675 https://github.com/esp-rs/esp-hal/issues/4675 | Maintainers note that C3 ECO3 and ECO7 have different ROM linker files and that esp-hal supports only one. | Open. esp-rom-sys links the **ECO3** set and comments out ECO7 (`R/esp-rom-sys-0.1.5/ld/esp32c3/rom-functions.x:2-5,21-25`). ECO3 is also IDF's default for REV_MIN v0.3, so a v0.4 chip (ECO6) is a supported combination. This does not by itself explain a v0.4 failure. |
| esp-hal PR #5700 (f562984d) and #5498 (37685900) | Pulse-reset of modem subsystems because "a system reset … leaves the modem domain intact … stale state trips controller asserts". | **Applies to C2/C5/C6/C61/H2 only.** C3 got nothing from it (`esp-radio/src/common_adapter.rs` in the esp-hal clone at `S/esp-hal`). |
| ESP-IDF v4.3.4 release notes https://github.com/espressif/esp-idf/releases/tag/v4.3.4 | "Fixed the task watchdog issue during controller initialization due to invalid hardware state after system reset on ESP32-C3". Also "Added support to use main crystal as Bluetooth sleep clock … on ESP32-C3". | IDF has the fix. esp-wifi's init path is a hand port that leaves those steps out (§3). |
| ESP32-C3 errata for v0.4 https://docs.espressif.com/projects/esp-chip-errata/en/latest/esp32c3/_tags/v0-4.html | Three errata only: ADC-270, CPU-863, ADC-183. | Nothing about the radio or BT. |
| esp32.com / arduino / esphome searches | Only generic C3 BLE+Wi-Fi coexistence complaints. | Nothing matching. |

No trouble or bt-hci issue matches. The hang happens before the first HCI byte crosses (below), so trouble and bt-hci are not involved.

## 2. The esp-wifi 0.15.1 BLE init path on esp32c3, and the exact semaphore

Order of calls (`R/esp-wifi-0.15.1/src/ble/btdm.rs:377-455`):
1. `btdm_controller_mem_init` → ROM `btdm_controller_rom_data_init` (`os_adapter_esp32c3.rs:309-313`).
2. `btdm_osi_funcs_register(&G_OSI_FUNCS)` (btdm.rs:397). `semphr_take` in that table is `crate::common_adapter::semphr_take` (os_adapter_esp32c3.rs:107, btdm.rs:140-147).
3. `bt_periph_module_enable()` and `disable_sleep_mode()` are **both empty** on C3 (`os_adapter_esp32c3.rs:315-321`).
4. `btdm_controller_init(&cfg)`, with `sleep_mode: 0, sleep_clock: 0` (`os_adapter_esp32c3.rs:253-296`). This call creates the controller task through the OSI `task_create` (btdm.rs:218-246).
5. `phy_enable()` (btdm.rs:436, `common_adapter_esp32c3.rs:71-102`). This is the same PHY path Wi-Fi uses, and Wi-Fi works.
6. **`btdm_controller_enable(ESP_BT_MODE_BLE)`** (btdm.rs:451). **This is where it hangs.**

Disassembly of `btdm_controller_enable` from the repo's own built probe (`research/dash/probes/target/riscv32imc-unknown-none-elf/release/ble-scan`, dumped to `S/ble-scan.dis`):
```
lw a5,-0x80(0x3fce0<<12) ; r_plf_funcs_p
lw a5,0x28(a5) ; a0=9,a1=0,a2=0,a3=1 ; jalr   -> post "enable" (msg 9) to the controller task
lw a5,-0x7c(..)          ; r_osi_funcs_p
lw a0,-0x44(..)          ; g_rw_init_sem
lw a5,0x34(a5) ; a1=-1 ; jalr   -> osi->semphr_take(g_rw_init_sem, OSI_FUNCS_TIME_BLOCKING)
```
Symbol addresses come from `R/esp-rom-sys-0.1.5/ld/esp32c3/rom/esp32c3.rom.ld:533,547,548`: `g_rw_init_sem = 0x3fcdffbc`, `r_osi_funcs_p = 0x3fcdff84`, `r_plf_funcs_p = 0x3fcdff80`. OSI offset `0x34` is field 13, `semphr_take` (`os_adapter_esp32c3.rs:18-91`).

- **Waited on:** `g_rw_init_sem`, a ROM global. The main task waits on it, inside `BleConnector::new`, with no timeout.
- **Should give it:** the BT controller task (ROM/blob code, handle `g_rw_controller_task_handle` at `0x3fcdffc0`). It gives the semaphore after it processes the enable message and brings up the LL (link layer). This is **a task, not an ISR** (inference from the ROM symbol names and the call pattern). The controller task in turn depends on the BT interrupts `RWBT`/`BT_BB` (source 5) and `RWBLE` (source 8). They are wired in `os_adapter_esp32c3.rs:348-386` and dispatched in `R/esp-wifi-0.15.1/src/radio/radio_esp32c3.rs:55-104`.
- Because the enable never returns, `API_vhci_host_register_callback` (btdm.rs:453) never runs. **No HCI command is ever sent.** The "first HCI command never completes" reading is the same hang.
- **Why the PC shows `semphr_take`.** `sem_take` loops on `yield_task()` (`compat/common.rs:226-247`). The main task sits there for good. The controller task is either spinning somewhere in ROM (0x4000_xxxx) or blocked in its own OSI call. **One PC sample cannot tell which.**

Chip-revision-dependent pieces:
- ROM symbols: ECO3 set only (`rom-functions.x`). The same combination as IDF's default build, which is valid for v0.3, v0.4 and v1.1.
- `hw_target_code: 0x01010000` is fixed per chip, not per revision (`os_adapter_esp32c3.rs:283`).
- The blob (`esp-wifi-sys-0.7.1/libs/esp32c3/libbtdm_app.a`, PHY build dated "Jun 4 2024") carries its own `r_*_eco` patch functions over ROM. It has no efuse or revision branch visible to `strings`/`nm`.
- The BT low-power clock default. `SYSTEM.BT_LPCK_DIV_FRAC` resets to `0x0200_1001` = `LPCLK_SEL_8M` (`R/esp32c3-0.33.0/src/system/bt_lpck_div_frac.rs:132-134`), and `BT_LPCK_DIV_INT` resets to `0xff`. esp-wifi never changes either register. RC_FAST frequency varies with the chip (it is an RC oscillator). **Inference:** this is a per-chip analog variable that IDF removes and esp-wifi keeps.

## 3. What IDF does before `btdm_controller_init` that esp-wifi 0.15.1 does not

From `components/bt/controller/esp32c3/bt.c` at v5.4.1 (local copy `S/idf-c3-bt.c`, source https://github.com/espressif/esp-idf/blob/v5.4.1/components/bt/controller/esp32c3/bt.c):

| IDF step (line) | esp-wifi 0.15.1 |
|---|---|
| `esp_bt_power_domain_on()`: clears `RTC_CNTL_BT_FORCE_PD/ISO` (482-490, 1430) | Done by esp-hal `init_clocks` (`R/esp-hal-1.0.0-rc.0/src/clock/clocks_ll/esp32c3.rs:206-216`). Equivalent. |
| `btdm_low_power_mode_init`: with sleep **off**, `lpclk_sel = MAIN_XTAL`, then `btdm_lpclk_select_src(BTDM_LPCLK_SEL_XTAL)` + `btdm_lpclk_set_div(xtal_MHz)` (1256, 1322-1331). This runs unconditionally. | **Missing.** The LP clock stays on the reset default (8M RC). Both functions are in the linked blob (`nm libbtdm_app.a`: `T btdm_lpclk_select_src`, `T btdm_lpclk_set_div`). They write `SYSTEM+0x24` and `SYSTEM+0x20`. |
| `periph_module_enable/reset(PERIPH_BT_MODULE)` (1463-1464). Reset mask = `BTBB_RST|BTBB_REG_RST|RW_BTMAC_RST|RW_BTLP_RST|RW_BTMAC_REG_RST|RW_BTLP_REG_RST` (`hal/esp32c3/include/hal/clk_gate_ll.h:101-102` at v5.4.1). Bits 3, 13, 9, 10, 11, 12 (`soc/esp32c3/register/soc/system_reg.h`/`syscon_reg.h`). | `bt_periph_module_enable` is empty. The only reset pulse is in `enable_wifi_power_domain` (`common_adapter_esp32c3.rs:16-50`): bits 0, 1, 2, 3, 4, 9, 11, 13. **Bits 10 and 12 (BT low-power module and its registers) are never pulsed.** |
| `sdk_config_extend_set_pll_track(false)` before enable (1646) | Missing. Probably harmless (inference). |

esp-radio master still has `bt_periph_module_enable() { /* nothing */ }` and `sleep_clock: 0` for C3/S3, and no lpclk call (`S/esp-hal/esp-radio/src/ble/btdm/os_adapter_esp32c3_s3.rs:535,610`; `grep lpclk` finds nothing). **An upgrade therefore does not bring these steps.**

## 4. Candidate fixes, ranked by cost

1. **Probe-level patch: the IDF steps in user code (minutes; no crate change).** Before `BleConnector::new`, pulse `APB_CTRL.WIFI_RST_EN` (0x6002_6018) with bits `3|9|10|11|12|13`, then call `btdm_lpclk_select_src(0)` and `btdm_lpclk_set_div(40)` from the blob. Both are `extern "C"` symbols. The diff is ready at `S/ble-scan-fix.diff`. It builds against the unchanged crate set: `S/probe-fix`, `BLE_FIX=1 CARGO_TARGET_DIR=target-fix cargo build --release --bin ble-scan`. If this works, the same few lines go into `vag-dash-fw` before `BleConnector::new`, and the repo keeps esp-hal rc.0.
2. **Rule the chip in or out with a known-good BLE firmware (about 1 h).** Run any ESP-IDF or Arduino-ESP32 BLE scan example on this board. If IDF BLE also fails, the chip or board is bad (clone modules have shipped marginal or rejected dies; inference) and no Rust change will help.
3. **esp-radio 0.16.0 (esp-hal `^1.0.0-rc.1`, esp-wifi-sys 0.8.1 blobs, esp-rtos 0.1 replaces the builtin scheduler and esp-hal-embassy)** (crates.io dependency API). This is the smallest upgrade: rc.0 → rc.1, which also unblocks esp-storage 0.8 (see the note at `crates/dash/vag-dash-fw/Cargo.toml:104-106`). Cost: moderate. Rename esp-wifi → esp-radio, move to the esp-rtos init, adapt to the rc.1 API, and bump trouble-host/bt-hci to the versions that pair with it. It brings newer controller blobs, but **not** the missing lpclk/BTLP steps. Its value for this bug is uncertain (inference).
4. **esp-radio 1.0.0-beta.1 (esp-hal `~1.2.0`, esp-rtos 0.4, esp-wifi-sys-esp32c3 0.3 = IDF 6.1 blobs)** (crates.io). This is the largest change: a whole esp-hal generation plus trouble-host 0.8 / bt-hci 0.10. It is also the only version upstream will accept a bug report against (see the #6335 closing comment).
5. **Hardware explanation.** Wi-Fi works at −55…−73 dBm, so the PHY, RF, 40 MHz crystal and power are fine. A BT-only defect would sit in the BT MAC or LP blocks, which Wi-Fi does not use. Only rank 2 can prove this.

No `ESP_WIFI_CONFIG_*` option in 0.15.1 touches the BT LP clock or the BT reset (`R/esp-wifi-0.15.1/esp_config.yml`), and there is no 0.15.x patch release after 0.15.1 (crates.io). `sys-logs` (the blob's own log) does not build on either installed toolchain: `VaListImpl` fails on both stable 1.98 and the installed nightly, as tried in `S/probe-fix`.

## 5. The experiment to run first

**Files** (already built, not flashed):
- `S/probe-fix/target-fix/riscv32imc-unknown-none-elf/release/ble-scan`: register dump, reset reason, **plus the fix**.
- `S/probe-fix/target-logs/riscv32imc-unknown-none-elf/release/ble-scan`: register dump plus `esp_wifi=trace`, **without the fix** (control run, shows the OSI calls up to the hang).
- `S/probe-fix/target/riscv32imc-unknown-none-elf/release/ble-scan`: register dump, no fix, info level.

**Steps:**
1. `espflash flash --monitor --chip esp32c3 S/probe-fix/target-fix/riscv32imc-unknown-none-elf/release/ble-scan`
2. Read the lines `[before] BT_LPCK_DIV_INT=… BT_LPCK_DIV_FRAC=… WIFI_RST_EN=…`, `BLE_FIX applied: select_src=true set_div=true`, `[after fix] …`, then `BleConnector::new returned` → `scanning` → `discovered …`.
3. Unplug the board, plug it back in, reattach the monitor, and repeat. The `reset reason` line tells a power-on start apart from a USB-JTAG or RWDT reset.

**Reading the result:**
- It scans → the root cause is the missing BTLP reset and/or LP clock selection. To find which step matters, comment one out and rerun.
- The control binary hangs only after a soft reset and works after a power cycle → stale BT hardware state across system resets (the IDF v4.3.4 class of bug). The BTLP reset is the fix.
- It still hangs → collect ~10 `Saved PC` values from the RWDT reboot loop of the full firmware, or halt with `probe-rs` over the board's built-in USB-JTAG and take a backtrace of the `btdm_controller_task` stack. Map any ROM PC (0x4000_xxxx) to the nearest symbol in `R/esp-rom-sys-0.1.5/ld/esp32c3/rom/*.ld`. That shows what the controller task is waiting for. Then run fix #2 to settle chip versus software.

(Registers dumped: `SYSTEM+0x20/0x24` = BT_LPCK_DIV_INT/FRAC, `APB_CTRL+0x14/0x18` = WIFI_CLK_EN/WIFI_RST_EN, `RTC_CNTL+0x88/0x8c/0x70` = DIG_PWC/DIG_ISO/CLK_CONF. Offsets checked against `R/esp32c3-0.33.0/src/{system,apb_ctrl,rtc_cntl}.rs`.)
