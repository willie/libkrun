// Copyright 2022 The ChromiumOS Authors
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

//! Implementation of the EDID specification provided by software.
//! EDID spec: <https://glenwing.github.io/docs/VESA-EEDID-A2.pdf>

//! This module is used to create the Extended Display Identification Data (EDID), which will be
//! exposed to the guest system.
//!
//! We ignore most of the spec, the point here being for us to provide enough for graphics to work
//! and to allow us to configure the resolution and refresh rate (via the preferred timing mode
//! pixel clock).
//!
//! The EDID spec defines a number of methods to provide mode information, but in priority order the
//! "detailed" timing information is first, so we provide a single block of detailed timing
//! information and no other form of timing information.
//!
use super::display::{EdidParams, PhysicalSize, MAX_DISPLAY_HEIGHT, MAX_DISPLAY_WIDTH};

const EDID_DATA_LENGTH: usize = 128;
const DEFAULT_HORIZONTAL_BLANKING: u16 = 560;
const DEFAULT_VERTICAL_BLANKING: u16 = 50;
const DEFAULT_HORIZONTAL_FRONT_PORCH: u16 = 64;
const DEFAULT_VERTICAL_FRONT_PORCH: u16 = 1;
const DEFAULT_HORIZONTAL_SYNC_PULSE: u16 = 192;
const DEFAULT_VERTICAL_SYNC_PULSE: u16 = 3;
const MILLIMETERS_PER_INCH: f32 = 25.4;

/// The highest refresh rate whose pixel clock the DisplayID timing holds (24
/// bits of 10 kHz, stored minus one) at the largest display size:
/// 2^24 * 10000 / ((8192 + 560) * (8192 + 50)) = 2325.
pub const MAX_REFRESH_RATE: u32 = ((1 << 24) * 10000
    / ((MAX_DISPLAY_WIDTH as u64 + DEFAULT_HORIZONTAL_BLANKING as u64)
        * (MAX_DISPLAY_HEIGHT as u64 + DEFAULT_VERTICAL_BLANKING as u64)))
    as u32;

#[derive(Copy, Clone)]
pub struct EdidInfo {
    width: u32,
    height: u32,
    refresh_rate: u32,
    horizontal_blanking: u16,
    vertical_blanking: u16,
    horizontal_front: u16,
    vertical_front: u16,
    horizontal_sync: u16,
    vertical_sync: u16,
    width_millimeters: u16,
    height_millimeters: u16,
}

impl EdidInfo {
    /// Only width, height and refresh rate are required for the graphics stack to work, so instead
    /// of pulling actual numbers from the system, we just use some typical values to populate other
    /// fields for now.
    pub fn new(width: u32, height: u32, params: &EdidParams) -> Self {
        let (width_millimeters, height_millimeters) = match params.physical_size {
            PhysicalSize::Dpi(dpi) => (
                ((width as f32 / dpi as f32) * MILLIMETERS_PER_INCH) as u16,
                ((height as f32 / dpi as f32) * MILLIMETERS_PER_INCH) as u16,
            ),
            PhysicalSize::DimensionsMillimeters(width, height) => (width, height),
        };

        Self {
            width,
            height,
            refresh_rate: params.refresh_rate,
            horizontal_blanking: DEFAULT_HORIZONTAL_BLANKING,
            vertical_blanking: DEFAULT_VERTICAL_BLANKING,
            horizontal_front: DEFAULT_HORIZONTAL_FRONT_PORCH,
            vertical_front: DEFAULT_VERTICAL_FRONT_PORCH,
            horizontal_sync: DEFAULT_HORIZONTAL_SYNC_PULSE,
            vertical_sync: DEFAULT_VERTICAL_SYNC_PULSE,
            width_millimeters,
            height_millimeters,
        }
    }

    pub fn width_centimeters(&self) -> u8 {
        (self.width_millimeters / 10) as u8
    }

    pub fn height_centimeters(&self) -> u8 {
        (self.height_millimeters / 10) as u8
    }

    /// The pixel clock in 10 kHz units.
    fn clock(&self) -> u64 {
        let htotal = self.width + u32::from(self.horizontal_blanking);
        let vtotal = self.height + u32::from(self.vertical_blanking);
        u64::from(self.refresh_rate) * u64::from(htotal) * u64::from(vtotal) / 10000
    }

    pub fn bytes(self) -> Box<[u8]> {
        let mut edid_box: Box<[u8]> = vec![0; 2 * EDID_DATA_LENGTH].into_boxed_slice();
        let (edid, extension) = edid_box.split_at_mut(EDID_DATA_LENGTH);

        populate_header(edid);
        populate_edid_version(edid);
        populate_size(edid, &self);
        populate_standard_timings(edid);

        // 4 available descriptor blocks. A detailed timing holds 12-bit
        // sizes and a 16-bit clock; a larger mode is only in the DisplayID
        // extension, and the descriptor is left as a dummy. Both timings have
        // the same clock, so the guest merges them into one mode.
        let block0 = &mut edid[54..72];
        if self.width < 4096 && self.height < 4096 && self.clock() <= u64::from(u16::MAX) {
            populate_detailed_timing(block0, &self);
        } else {
            block0[3] = 0x10;
        }

        let block1 = &mut edid[72..90];
        populate_display_name(block1);

        edid[126] = 1;
        calculate_checksum(edid);

        populate_displayid(extension, &self);
        calculate_checksum(extension);

        edid_box
    }
}

fn populate_display_name(edid_block: &mut [u8]) {
    // Display Product Name String Descriptor Tag
    edid_block[0..5].clone_from_slice(&[0x00, 0x00, 0x00, 0xFC, 0x00]);
    // This should to be padded to 13 bytes, see Section 3.10.3.4
    let product_name: &[u8; 13] = b"krun-display\n";
    edid_block[5..].clone_from_slice(product_name);
}

fn populate_detailed_timing(edid_block: &mut [u8], info: &EdidInfo) {
    assert_eq!(edid_block.len(), 18);

    // Detailed timings
    //
    // 18 Byte Descriptors - 72 Bytes
    // The 72 bytes in this section are divided into four data fields. Each of the four data fields
    // are 18 bytes in length. These 18 byte data fields shall contain either detailed timing data
    // as described in Section 3.10.2 or other types of data as described in Section 3.10.3. The
    // addresses and the contents of the four 18 byte descriptors are shown in Table 3.20.
    //
    // We leave the bottom 6 bytes of this block purposefully empty.
    let horizontal_blanking_lsb: u8 = (info.horizontal_blanking & 0xFF) as u8;
    let horizontal_blanking_msb: u8 = ((info.horizontal_blanking >> 8) & 0x0F) as u8;

    let vertical_blanking_lsb: u8 = (info.vertical_blanking & 0xFF) as u8;
    let vertical_blanking_msb: u8 = ((info.vertical_blanking >> 8) & 0x0F) as u8;

    // The pixel clock is what controls the refresh timing information.
    //
    // The formula for getting refresh rate out of this value is:
    //   refresh_rate = clk * 10000 / (htotal * vtotal)
    // Solving for clk:
    //   clk = (refresh_rate * htotal * votal) / 10000
    //
    // where:
    //   clk - The setting here
    //   vtotal - Total lines
    //   htotal - Total pixels per line
    //
    // Value here is the pixel clock in 10 kHz steps, as in the DisplayID
    // timing.
    //
    // Pseudocode of kernel logic for vrefresh:
    //    vtotal := mode->vtotal;
    //    calc_val := (clock * 1000) / htotal
    //    refresh := (calc_val + vtotal / 2) / vtotal
    //    if flags & INTERLACE: refresh *= 2
    //    if flags & DBLSCAN: refresh /= 2
    //    if vscan > 1: refresh /= vscan
    //
    let clock = info.clock() as u16;
    edid_block[0..2].copy_from_slice(&clock.to_le_bytes());

    let width_lsb: u8 = (info.width & 0xFF) as u8;
    let width_msb: u8 = ((info.width >> 8) & 0x0F) as u8;

    // Horizointal Addressable Video in pixels.
    edid_block[2] = width_lsb;
    // Horizontal blanking in pixels.
    edid_block[3] = horizontal_blanking_lsb;
    // Upper bits of the two above vals.
    edid_block[4] = horizontal_blanking_msb | (width_msb << 4);

    let vertical_active: u32 = info.height;
    let vertical_active_lsb: u8 = (vertical_active & 0xFF) as u8;
    let vertical_active_msb: u8 = ((vertical_active >> 8) & 0x0F) as u8;

    // Vertical addressable video in *lines*
    edid_block[5] = vertical_active_lsb;
    // Vertical blanking in lines
    edid_block[6] = vertical_blanking_lsb;
    // Sigbits of the above.
    edid_block[7] = vertical_blanking_msb | (vertical_active_msb << 4);

    let horizontal_front_lsb: u8 = (info.horizontal_front & 0xFF) as u8; // least sig 8 bits
    let horizontal_front_msb: u8 = ((info.horizontal_front >> 8) & 0x03) as u8; // most sig 2 bits
    let horizontal_sync_lsb: u8 = (info.horizontal_sync & 0xFF) as u8; // least sig 8 bits
    let horizontal_sync_msb: u8 = ((info.horizontal_sync >> 8) & 0x03) as u8; // most sig 2 bits

    let vertical_front_lsb: u8 = (info.vertical_front & 0x0F) as u8; // least sig 4 bits
    let vertical_front_msb: u8 = ((info.vertical_front >> 8) & 0x0F) as u8; // most sig 2 bits
    let vertical_sync_lsb: u8 = (info.vertical_sync & 0xFF) as u8; // least sig 4 bits
    let vertical_sync_msb: u8 = ((info.vertical_sync >> 8) & 0x0F) as u8; // most sig 2 bits

    // Horizontal front porch in pixels.
    edid_block[8] = horizontal_front_lsb;
    // Horizontal sync pulse width in pixels.
    edid_block[9] = horizontal_sync_lsb;
    // LSB of vertical front porch and sync pulse
    edid_block[10] = vertical_sync_lsb | (vertical_front_lsb << 4);
    // Upper 2 bits of these values.
    edid_block[11] = vertical_sync_msb
        | (vertical_front_msb << 2)
        | (horizontal_sync_msb << 4)
        | (horizontal_front_msb << 6);

    let width_millimeters_lsb: u8 = (info.width_millimeters & 0xFF) as u8; // least sig 8 bits
    let width_millimeters_msb: u8 = ((info.width_millimeters >> 8) & 0xF) as u8; // most sig 4 bits

    let height_millimeters_lsb: u8 = (info.height_millimeters & 0xFF) as u8; // least sig 8 bits
    let height_millimeters_msb: u8 = ((info.height_millimeters >> 8) & 0xF) as u8; // most sig 4 bits

    edid_block[12] = width_millimeters_lsb;
    edid_block[13] = height_millimeters_lsb;
    edid_block[14] = height_millimeters_msb | (width_millimeters_msb << 4);
}

// A DisplayID 1.3 extension block with the mode as its one, preferred, Type I
// detailed timing: 16-bit sizes and a 24-bit clock, for modes a detailed
// timing in the base block can't hold. Every field is stored minus one.
fn populate_displayid(extension: &mut [u8], info: &EdidInfo) {
    const TIMING_LENGTH: u8 = 20;
    const BLOCK_LENGTH: u8 = 3 + TIMING_LENGTH;

    extension[0] = 0x70; // DisplayID extension
    let section = &mut extension[1..];
    section[0] = 0x13; // version 1.3
    section[1] = BLOCK_LENGTH;
    section[2] = 0x00; // extension section
    section[3] = 0; // no further sections

    let block = &mut section[4..4 + usize::from(BLOCK_LENGTH)];
    block[0] = 0x03; // Type I detailed timing
    block[1] = 0x00;
    block[2] = TIMING_LENGTH;
    let timing = &mut block[3..];
    timing[0..3].copy_from_slice(&(info.clock() - 1).to_le_bytes()[..3]);
    timing[3] = 0x80; // preferred
    let fields = [
        info.width,
        u32::from(info.horizontal_blanking),
        u32::from(info.horizontal_front),
        u32::from(info.horizontal_sync),
        info.height,
        u32::from(info.vertical_blanking),
        u32::from(info.vertical_front),
        u32::from(info.vertical_sync),
    ];
    for (i, field) in fields.into_iter().enumerate() {
        timing[4 + 2 * i..6 + 2 * i].copy_from_slice(&((field - 1) as u16).to_le_bytes());
    }

    let end = 4 + usize::from(BLOCK_LENGTH);
    let sum = section[..end].iter().fold(0u8, |sum, byte| sum.wrapping_add(*byte));
    section[end] = sum.wrapping_neg();
}

// The EDID header. This is defined by the EDID spec.
fn populate_header(edid: &mut [u8]) {
    edid[0] = 0x00;
    edid[1] = 0xFF;
    edid[2] = 0xFF;
    edid[3] = 0xFF;
    edid[4] = 0xFF;
    edid[5] = 0xFF;
    edid[6] = 0xFF;
    edid[7] = 0x00;

    // Red Hat 'RHT' is also used in QEMU, though it is not technically officially assigned
    let manufacturer_name = b"RHT";
    // 00001 -> A, 00010 -> B, etc
    let manufacturer_id: u16 = manufacturer_name
        .iter()
        .map(|c| (c - b'A' + 1) & 0x1F)
        .fold(0u16, |res, lsb| (res << 5) | (lsb as u16));
    edid[8..10].copy_from_slice(&manufacturer_id.to_be_bytes());

    let manufacture_product_id: u16 = 1;
    edid[10..12].copy_from_slice(&manufacture_product_id.to_le_bytes());

    let serial_id: u32 = 1;
    edid[12..16].copy_from_slice(&serial_id.to_le_bytes());

    let manufacture_week: u8 = 30;
    edid[16] = manufacture_week;

    let manufacture_year: u32 = 2025;
    edid[17] = (manufacture_year - 1990u32) as u8;
}

// The 8 standard timings, all unused (01 01). A mode besides the requested
// one stays offered after the display is resized, and a desktop that once
// took it keeps it: GNOME's mutter reapplies its current configuration while
// that mode is still listed, whatever the preferred mode is.
fn populate_standard_timings(edid: &mut [u8]) {
    edid[0x26..0x36].fill(0x01);
}

// Per the EDID spec, needs to be 1 and 4.
fn populate_edid_version(edid: &mut [u8]) {
    edid[18] = 1;
    edid[19] = 4;
}

fn populate_size(edid: &mut [u8], info: &EdidInfo) {
    edid[21] = info.width_centimeters();
    edid[22] = info.height_centimeters();
}

fn calculate_checksum(edid: &mut [u8]) {
    let mut checksum: u8 = 0;
    for byte in edid.iter().take(EDID_DATA_LENGTH - 1) {
        checksum = checksum.wrapping_add(*byte);
    }

    if checksum != 0 {
        checksum = 255 - checksum + 1;
    }

    edid[127] = checksum;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn displayid_clock_of_a_large_fast_mode() {
        let params = EdidParams {
            refresh_rate: 240,
            physical_size: PhysicalSize::Dpi(110),
        };
        let edid = EdidInfo::new(7680, 2160, &params).bytes();
        // The extension's Type I timing: its clock in 10 kHz units, minus one.
        let timing = &edid[EDID_DATA_LENGTH + 8..];
        let clock = u32::from_le_bytes([timing[0], timing[1], timing[2], 0]) + 1;
        assert_eq!(clock, 437_049); // 240 Hz * (7680 + 560) * (2160 + 50) / 10000
    }

    #[test]
    fn only_the_requested_mode() {
        let params = EdidParams {
            refresh_rate: 60,
            physical_size: PhysicalSize::Dpi(110),
        };
        let edid = EdidInfo::new(1920, 1200, &params).bytes();
        // No established or standard timings.
        assert_eq!(edid[0x23..0x26], [0; 3]);
        assert!(edid[0x26..0x36].iter().all(|&b| b == 0x01));
    }

    /// A mode as Linux 7.2 decodes it from an EDID (drm_edid.c): the fields
    /// drm_mode_equal compares, and whether the mode is preferred.
    #[derive(Debug, Clone, Copy)]
    struct Mode {
        /// kHz
        clock: u32,
        /// display, sync start, sync end, total
        h: [u32; 4],
        v: [u32; 4],
        phsync: bool,
        pvsync: bool,
        interlace: bool,
        preferred: bool,
    }

    impl Mode {
        /// drm_mode_equal: the timings, the flags, and the clock as a period
        /// in picoseconds (KHZ2PICOS).
        fn equal(&self, other: &Mode) -> bool {
            self.h == other.h
                && self.v == other.v
                && (self.phsync, self.pvsync, self.interlace)
                    == (other.phsync, other.pvsync, other.interlace)
                && 1_000_000_000 / self.clock == 1_000_000_000 / other.clock
        }

        fn refresh(&self) -> f64 {
            f64::from(self.clock) * 1000.0 / f64::from(self.h[3] * self.v[3])
        }
    }

    /// drm_mode_detailed for the base block's first descriptor, which
    /// add_detailed_modes makes preferred in an EDID 1.4.
    fn detailed_mode(edid: &[u8]) -> Option<Mode> {
        let d = &edid[54..72];
        let clock = u16::from_le_bytes([d[0], d[1]]);
        // is_detailed_timing_descriptor
        if clock == 0 {
            return None;
        }
        let hactive = u32::from(d[4] & 0xf0) << 4 | u32::from(d[2]);
        let vactive = u32::from(d[7] & 0xf0) << 4 | u32::from(d[5]);
        let hblank = u32::from(d[4] & 0x0f) << 8 | u32::from(d[3]);
        let vblank = u32::from(d[7] & 0x0f) << 8 | u32::from(d[6]);
        let hsync_offset = u32::from(d[11] & 0xc0) << 2 | u32::from(d[8]);
        let hsync_width = u32::from(d[11] & 0x30) << 4 | u32::from(d[9]);
        let vsync_offset = u32::from(d[11] & 0x0c) << 2 | u32::from(d[10] >> 4);
        let vsync_width = u32::from(d[11] & 0x03) << 4 | u32::from(d[10] & 0x0f);
        let misc = d[17];
        if hactive < 64 || vactive < 64 || misc & (1 << 5) != 0 {
            return None;
        }
        if hsync_width == 0 || vsync_width == 0 {
            return None;
        }
        let htotal = hactive + hblank;
        let vtotal = vactive + vblank;
        let hsync_start = hactive + hsync_offset;
        let vsync_start = vactive + vsync_offset;
        // Interlaced CEA sizes are doubled; none is generated.
        assert_eq!(misc & (1 << 7), 0);
        Some(Mode {
            clock: u32::from(clock) * 10,
            h: [
                hactive,
                hsync_start,
                (hsync_start + hsync_width).min(htotal),
                htotal,
            ],
            v: [
                vactive,
                vsync_start,
                (vsync_start + vsync_width).min(vtotal),
                vtotal,
            ],
            phsync: misc & (1 << 1) != 0,
            pvsync: misc & (1 << 2) != 0,
            interlace: false,
            preferred: edid[19] >= 4,
        })
    }

    /// drm_mode_displayid_detailed for each Type I timing in the DisplayID
    /// extension.
    fn displayid_modes(edid: &[u8]) -> Vec<Mode> {
        assert_eq!(edid[126], 1);
        let extension = &edid[EDID_DATA_LENGTH..];
        assert_eq!(extension[0], 0x70);
        let section_end = 5 + usize::from(extension[2]);
        let mut modes = Vec::new();
        let mut block = 5;
        while block < section_end {
            let (tag, length) = (extension[block], usize::from(extension[block + 2]));
            if tag == 0x03 {
                assert_eq!(length % 20, 0);
                for t in extension[block + 3..block + 3 + length].chunks(20) {
                    let field = |i: usize| u32::from(u16::from_le_bytes([t[i], t[i + 1]]));
                    let hactive = field(4) + 1;
                    let vactive = field(12) + 1;
                    let hsync_start = hactive + (field(8) & 0x7fff) + 1;
                    let vsync_start = vactive + (field(16) & 0x7fff) + 1;
                    modes.push(Mode {
                        clock: (u32::from_le_bytes([t[0], t[1], t[2], 0]) + 1) * 10,
                        h: [
                            hactive,
                            hsync_start,
                            hsync_start + field(10) + 1,
                            hactive + field(6) + 1,
                        ],
                        v: [
                            vactive,
                            vsync_start,
                            vsync_start + field(18) + 1,
                            vactive + field(14) + 1,
                        ],
                        phsync: field(8) & 0x8000 != 0,
                        pvsync: field(16) & 0x8000 != 0,
                        interlace: false,
                        preferred: t[3] & 0x80 != 0,
                    });
                }
            }
            block += 3 + length;
        }
        modes
    }

    /// The guest decodes `descriptors` modes from the EDID for the size and
    /// rate, and, merging equal ones as drm_connector_list_update does, lists
    /// one: the size, preferred, at the rate to within a 10 kHz clock step.
    fn assert_one_mode(width: u32, height: u32, refresh_rate: u32, descriptors: usize) {
        let params = EdidParams {
            refresh_rate,
            physical_size: PhysicalSize::Dpi(110),
        };
        let edid = EdidInfo::new(width, height, &params).bytes();
        let probed: Vec<Mode> = detailed_mode(&edid)
            .into_iter()
            .chain(displayid_modes(&edid))
            .collect();
        let name = format!("{width}x{height}@{refresh_rate}");
        assert_eq!(probed.len(), descriptors, "{name}: {probed:?}");
        let mut listed: Vec<Mode> = Vec::new();
        for mode in &probed {
            if !listed.iter().any(|m| m.equal(mode)) {
                listed.push(*mode);
            }
        }
        assert_eq!(listed.len(), 1, "{name}: {probed:?}");
        assert!(probed.iter().all(|m| m.preferred), "{name}: {probed:?}");
        let mode = listed[0];
        assert_eq!((mode.h[0], mode.v[0]), (width, height), "{name}");
        let step = 10_000.0 / f64::from(mode.h[3] * mode.v[3]);
        let refresh = mode.refresh();
        assert!(
            refresh <= f64::from(refresh_rate) && f64::from(refresh_rate) - refresh < step,
            "{name}: {refresh} Hz"
        );
    }

    #[test]
    fn one_mode_when_the_detailed_timing_fits() {
        for (width, height, refresh_rate) in [
            (1366, 768, 60),
            (1920, 1080, 60),
            (1920, 1200, 60),
            (2560, 1440, 60),
            (3024, 1964, 60),
        ] {
            assert_one_mode(width, height, refresh_rate, 2);
        }
    }

    #[test]
    fn one_mode_when_the_detailed_timing_does_not_fit() {
        for (width, height, refresh_rate) in [
            // 120 Hz * (3024 + 560) * (1964 + 50) / 10000 = 86618: past 16 bits.
            (3024, 1964, 120),
            (5120, 2880, 60),
            (7680, 4320, 60),
            (2560, 1440, 240),
        ] {
            assert_one_mode(width, height, refresh_rate, 1);
        }
    }

    #[test]
    fn detailed_timing_clock_at_the_16_bit_limit() {
        // 60 Hz * (3021 + 560) * (3000 + 50) / 10000 = 65532
        assert_one_mode(3021, 3000, 60, 2);
        // 60 Hz * (3022 + 560) * (3000 + 50) / 10000 = 65550
        assert_one_mode(3022, 3000, 60, 1);
    }

    #[test]
    fn one_mode_at_every_size_and_rate() {
        // 1 to 8192 by 8192 and 1 to 2325 Hz: the limits of krun_add_display,
        // the display backend and krun_display_set_refresh_rate.
        for width in (1..=8192).step_by(127).chain([8192]) {
            for height in (1..=8192).step_by(113).chain([8192]) {
                for refresh_rate in [1, 60, 120, 2325] {
                    let params = EdidParams {
                        refresh_rate,
                        physical_size: PhysicalSize::Dpi(110),
                    };
                    let edid = EdidInfo::new(width, height, &params).bytes();
                    let descriptors =
                        usize::from(detailed_mode(&edid).is_some()) + displayid_modes(&edid).len();
                    assert_one_mode(width, height, refresh_rate, descriptors);
                }
            }
        }
    }
}
