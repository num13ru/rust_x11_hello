# USBNetwork verification on current Kindle

Date: 2026-08-30
Status: **BLOCKED — no compatible USBNetwork package exists for this device**
(The bundled repo package is obsolete; the current maintained K5 package's device
allowlist predates this hardware and MRPI correctly refuses it.)

## Objective

Verify that the USBNetwork hack works with the current Kindle. MTP tooling was
reused from `/Users/user/GIT/rust_x11_hello` (`mtp-rs` at `~/.cargo/bin/mtp-rs`).

## Device — Amazon Kindle Paperwhite 6 (12th gen), NOT PW5

- Serial `GN433X11518401E8` → WinterBreak serial-prefix legend: `GN4` = Paperwhite 6 (Sangria)
- Device code (devcode, reported by MRPI/FBInk): **`C7F`**
- FBInk `fbink_device_id.c` (master) maps `0xC7F` in the group:
  ```
  case 0xC89u: case 0xC86u: case 0xC7Fu: case 0xC7Eu:
  case 0xE2Au: case 0xE25u: case 0xE23u: case 0xE28u:
  case 0xE45u: case 0xE5Au:
      deviceName  = "PaperWhite 6"
      deviceCodename = "Sangria"
      devicePlatform = "Bellatrix4"
  ```
- Firmware: `Kindle 5.17.1.0.4 (435197 007)` (from rust_x11_hello phase 3 test record)
- Jailbreak: WinterBreak 1.7.0 (log: `/winterbreak.log`, `jb.sh`); developer key + `MNTUS_EXEC` installed
- KUAL + MRInstaller (MRPI r19303) present; kterm, KOReader, alpine, etc. present
- Earlier X11 runs recorded screen `1272x1696` and labelled the device "PW5"; the
  serial+devcode evidence above identifies it as **PW6**.

## Repo state (kindle-usbnetwork)

- This repo ships USBNetwork **v0.33.N** (`src/usbnet`), last ChangeLog entry 2011-04-19.
- Targets **Kindle 2/DX/DXG/3** only: `build-updates.sh` model list
  `k2 k2i dx dxi dxg k3g k3w k3gb`.
- Binaries: 2011 ARM EABI5, dynamically linked against `/lib/ld-linux.so.3`
  (verified with `file`). Incompatible/unsupported on K5 (FW 5.x) — package is obsolete for this device.

## Package selection & install attempt

Candidates:

1. `~/Downloads/kindle-usbnetwork-0.57.N-k4.zip` — user-downloaded.
   - Legacy thread (K2/DX/K3/4), targets **K4** (non-touch), 2015.
   - **Wrong family** for this device — rejected.
2. `kindle-usbnet-0.22.N-r19297.tar.xz` — NiLuJe K5 snapshots thread
   (`mobileread.com/forums/showthread.php?t=225030`), Nov 2023.
   - Covers K5 family up to PW5 (`-d ... paperwhite5 basic4 scribe` in `build-updates.sh`).
   - `Update_usbnet_0.22.N_install_pw2_and_up.bin` uploaded to `/mrpackages/`
     with `mtp-rs put --verify` (15,207,559 bytes, readback verified).

MRPI attempt (via KUAL → MR Installer → Helper → Install MR Packages):

- Log `extensions/MRInstaller/log/mrinstaller.log` proves the run:
  ```
  [2026-08-30 10:27:38] :: Processing 'Update_usbnet_0.22.N_install_pw2_and_up.bin'
     (usbnet 0.22.N I W+Z+R+B) ...
  Package ... is not targeting your device [C7F vs. D4 5A D5 ... 6FF 971 9B3 82A 958 957 847 875 874],
     skipping . . . :(
  ```
- Device devcode **C7F** is not in the package's allowlist (which ends at PW5 0x9B3,
  Basic 4, Scribe — no PW6 0xC7F). MRPI checks `kindletool` device list; refusal is correct.
- MRPI removed the staged `.bin` from `/mrpackages/` after the failed attempt (dir now empty).

## Why 0.22.N does not cover PW6

- Package built 2023-11-06 (`build-updates.sh` PKGVER 0.22.N), device flags end at
  `-d paperwhite5 -d basic4 -d scribe`.
- PW6 (Sangria/Bellatrix4) devcodes (0xC89/C86/C7F/C7E/E2A/E25/E23/E28/E45/E5A)
  are recognized only in **current** FBInk master (post-2023); the package's bundled
  `libkh5`/`fbink`/`usbnet.sh` have no `IS_PW6`/Sangria branch and no PW6 screen size.
- Snapshots thread page 192 (2026 posts) shows users still directed to
  kindlemodding.org for "newer jailbreaks"; the current NiLuJe thread does not list
  a PW6-capable USBNetwork release. [INFERENCE: no maintained PW6 build exists yet]

## Facts established (verified)

- MRPI works and logs; install failed solely on device allowlist (C7F vs. list), not on
  packaging/signing or on MRPI itself.
- The device is Paperwhite 6 (Sangria / Bellatrix4), serial-prefix GN4 + devcode C7F
  (FBInk master).
- The maintained K5 USBNetwork 0.22.N (Nov 2023) does not support PW6.
- No newer official K5 usbnet build is published in the snapshots thread.
- The bundled 0.33.N in this repo targets K2/K3 only and is obsolete for this device.

## Result

- **Not verified / not installable with current official tooling.**
- The task "verify usbnetwork works with current kindle" cannot be completed as-is:
  the only supported package for the K5 family does not list this device, and MRPI
  correctly blocks installation.
- Options (user decision):
  1. Wait for a PW6-capable USBNetwork release (NiLuJe snapshots / kindlemodding.org).
  2. If a PW6-capable usbnet build appears, re-upload to `/mrpackages/` and re-run MRPI.
  3. Manual fallback (not attempted, unsafe): extract `Update_usbnet_0.22.N_install_*`
     and hand-place `/mnt/us/usbnet` + `/usr/local/bin` scripts/bins. High risk on
     new SoC/firmware; not recommended.
- Device left in a clean state: no partial usbnet install, `/mrpackages/` empty,
  MRPI logs intact.
