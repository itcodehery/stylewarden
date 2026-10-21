//! Standard MIDI File (SMF0 / SMF1) parser, section slicer, and SFF1 SMF0 compiler.
//!
//! Ensures strict compliance with Yamaha PSR-E583 SFF1 requirements:
//! - Format 0 (`MThd` format 0, 1 track)
//! - Measure 1 (Initialization): No musical notes. At tick 0, Meta Markers `SFF1` and `SInt`,
//!   Time Signature, Tempo, GM/XG System On SysEx, and per-channel Bank Select (`CC#0`, `CC#32`),
//!   Program Change, Volume (`CC#7`), Pan (`CC#10`), Reverb (`CC#91`), and Chorus (`CC#93`).
//! - Measure 2 onwards: Section markers (`Intro A`, `Main A`, `Main B`, `Fill In AA`, etc.)
//!   placed on exact bar boundaries, with note-offs clamped cleanly at section seams.

use crate::casm::{AccompanimentPart, StyleSection};
use serde::{Deserialize, Serialize};

/// A single MIDI note event with tick position and duration.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct MidiNote {
    /// Tick offset relative to the start of the section (or file when importing).
    pub start_tick: u32,
    /// Duration in ticks.
    pub duration_ticks: u32,
    /// MIDI channel (0..=15).
    pub channel: u8,
    /// MIDI note number (0..=127).
    pub note: u8,
    /// Note-on velocity (1..=127).
    pub velocity: u8,
}

/// Low-level SMF event for sorting and encoding into `MTrk`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RawSmfEvent {
    Meta {
        tick: u32,
        meta_type: u8,
        data: Vec<u8>,
    },
    SysEx {
        tick: u32,
        data: Vec<u8>, // Payload after F0, including trailing F7
    },
    ControlChange {
        tick: u32,
        channel: u8,
        controller: u8,
        value: u8,
    },
    ProgramChange {
        tick: u32,
        channel: u8,
        program: u8,
    },
    NoteOff {
        tick: u32,
        channel: u8,
        note: u8,
        velocity: u8,
    },
    NoteOn {
        tick: u32,
        channel: u8,
        note: u8,
        velocity: u8,
    },
}

impl RawSmfEvent {
    pub fn tick(&self) -> u32 {
        match self {
            RawSmfEvent::Meta { tick, .. }
            | RawSmfEvent::SysEx { tick, .. }
            | RawSmfEvent::ControlChange { tick, .. }
            | RawSmfEvent::ProgramChange { tick, .. }
            | RawSmfEvent::NoteOff { tick, .. }
            | RawSmfEvent::NoteOn { tick, .. } => *tick,
        }
    }

    /// Priority at identical ticks: Meta -> SysEx -> CC -> PC -> NoteOff -> NoteOn.
    /// Ensures Bank Select / Program Change happen before NoteOn, and NoteOff precedes NoteOn on re-articulation.
    pub fn sort_rank(&self) -> u8 {
        match self {
            RawSmfEvent::Meta { meta_type, .. } => {
                if *meta_type == 0x2F {
                    99 // End of Track always last
                } else {
                    0
                }
            }
            RawSmfEvent::SysEx { .. } => 1,
            RawSmfEvent::ControlChange { .. } => 2,
            RawSmfEvent::ProgramChange { .. } => 3,
            RawSmfEvent::NoteOff { .. } => 4,
            RawSmfEvent::NoteOn { .. } => 5,
        }
    }
}

/// Encodes a u32 into a MIDI Variable-Length Quantity (VLQ).
pub fn write_vlq(mut val: u32, out: &mut Vec<u8>) {
    let mut buf = [0u8; 5];
    let mut idx = 4;
    buf[idx] = (val & 0x7F) as u8;
    val >>= 7;
    while val > 0 {
        idx -= 1;
        buf[idx] = ((val & 0x7F) as u8) | 0x80;
        val >>= 7;
    }
    out.extend_from_slice(&buf[idx..=4]);
}

/// Reads a MIDI Variable-Length Quantity (VLQ) from `data` starting at `*pos`.
pub fn read_vlq(data: &[u8], pos: &mut usize) -> Option<u32> {
    let mut val = 0u32;
    for _ in 0..4 {
        if *pos >= data.len() {
            return None;
        }
        let b = data[*pos];
        *pos += 1;
        val = (val << 7) | ((b & 0x7F) as u32);
        if (b & 0x80) == 0 {
            return Some(val);
        }
    }
    Some(val)
}

/// Summary of an imported `.mid` or `.sty` SMF section.
#[derive(Debug, Clone)]
pub struct ParsedSmf {
    pub format: u16,
    pub num_tracks: u16,
    pub ticks_per_quarter: u16,
    pub bpm: f32,
    pub time_sig_num: u8,
    pub time_sig_den: u8,
    pub markers: Vec<(u32, String)>,
    pub notes: Vec<MidiNote>,
    pub smf_end_offset: usize,
}

impl ParsedSmf {
    pub fn ticks_per_bar(&self) -> u32 {
        let num = self.time_sig_num.max(1) as u32;
        let den = self.time_sig_den.max(1) as u32;
        (self.ticks_per_quarter as u32) * num * 4 / den
    }

    pub fn total_bars(&self) -> usize {
        let max_tick = self
            .notes
            .iter()
            .map(|n| n.start_tick + n.duration_ticks)
            .chain(self.markers.iter().map(|(t, _)| *t))
            .max()
            .unwrap_or(0);
        let tpb = self.ticks_per_bar().max(1);
        max_tick.div_ceil(tpb) as usize
    }
}

/// Parses a Standard MIDI File (Format 0 or Format 1) or the MIDI half of a `.sty` file.
pub fn parse_smf_bytes(data: &[u8]) -> Result<ParsedSmf, String> {
    if data.len() < 14 || &data[0..4] != b"MThd" {
        return Err("Missing 'MThd' MIDI header".to_string());
    }
    let header_len = u32::from_be_bytes([data[4], data[5], data[6], data[7]]) as usize;
    if header_len < 6 || 8 + header_len > data.len() {
        return Err("Invalid MThd header length".to_string());
    }
    let format = u16::from_be_bytes([data[8], data[9]]);
    let num_tracks = u16::from_be_bytes([data[10], data[11]]);
    let division = u16::from_be_bytes([data[12], data[13]]);
    if (division & 0x8000) != 0 {
        return Err("SMPTE timecode division is not supported".to_string());
    }
    let ticks_per_quarter = division.max(1);

    let mut cursor = 8 + header_len;
    let mut bpm = 120.0f32;
    let mut time_sig_num = 4u8;
    let mut time_sig_den = 4u8;
    let mut markers = Vec::new();
    let mut notes = Vec::new();

    for _ in 0..num_tracks {
        if cursor + 8 > data.len() {
            break;
        }
        if &data[cursor..cursor + 4] != b"MTrk" {
            break;
        }
        let trk_len =
            u32::from_be_bytes([data[cursor + 4], data[cursor + 5], data[cursor + 6], data[cursor + 7]])
                as usize;
        cursor += 8;
        let trk_end = (cursor + trk_len).min(data.len());
        let trk_slice = &data[cursor..trk_end];

        let mut pos = 0usize;
        let mut tick = 0u32;
        let mut running_status = 0u8;
        // Active notes map: (channel, note) -> (start_tick, velocity)
        let mut active: std::collections::HashMap<(u8, u8), (u32, u8)> =
            std::collections::HashMap::new();

        while pos < trk_slice.len() {
            let Some(delta) = read_vlq(trk_slice, &mut pos) else {
                break;
            };
            tick = tick.saturating_add(delta);
            if pos >= trk_slice.len() {
                break;
            }

            let status = if trk_slice[pos] >= 0x80 {
                let s = trk_slice[pos];
                pos += 1;
                if s < 0xF0 {
                    running_status = s;
                }
                s
            } else {
                running_status
            };

            if status == 0xFF {
                if pos >= trk_slice.len() {
                    break;
                }
                let meta_type = trk_slice[pos];
                pos += 1;
                let Some(len) = read_vlq(trk_slice, &mut pos) else {
                    break;
                };
                let len = len as usize;
                if pos + len > trk_slice.len() {
                    break;
                }
                let payload = &trk_slice[pos..pos + len];
                pos += len;

                match meta_type {
                    0x06 => {
                        let text = String::from_utf8_lossy(payload).to_string();
                        markers.push((tick, text));
                    }
                    0x51 if payload.len() == 3 => {
                        let us_per_qn = ((payload[0] as u32) << 16)
                            | ((payload[1] as u32) << 8)
                            | (payload[2] as u32);
                        if us_per_qn > 0 {
                            bpm = (60_000_000.0 / (us_per_qn as f32)).clamp(20.0, 400.0);
                        }
                    }
                    0x58 if payload.len() >= 2 => {
                        time_sig_num = payload[0].max(1);
                        time_sig_den = 1u8.checked_shl(payload[1] as u32).unwrap_or(4);
                    }
                    0x2F => {
                        break;
                    }
                    _ => {}
                }
            } else if status == 0xF0 || status == 0xF7 {
                let Some(len) = read_vlq(trk_slice, &mut pos) else {
                    break;
                };
                pos = (pos + len as usize).min(trk_slice.len());
            } else {
                let cmd = status & 0xF0;
                let ch = status & 0x0F;
                match cmd {
                    0x80 => {
                        if pos + 2 > trk_slice.len() {
                            break;
                        }
                        let note = trk_slice[pos] & 0x7F;
                        pos += 2;
                        if let Some((start, vel)) = active.remove(&(ch, note)) {
                            notes.push(MidiNote {
                                start_tick: start,
                                duration_ticks: tick.saturating_sub(start).max(1),
                                channel: ch,
                                note,
                                velocity: vel,
                            });
                        }
                    }
                    0x90 => {
                        if pos + 2 > trk_slice.len() {
                            break;
                        }
                        let note = trk_slice[pos] & 0x7F;
                        let vel = trk_slice[pos + 1] & 0x7F;
                        pos += 2;
                        if vel == 0 {
                            if let Some((start, on_vel)) = active.remove(&(ch, note)) {
                                notes.push(MidiNote {
                                    start_tick: start,
                                    duration_ticks: tick.saturating_sub(start).max(1),
                                    channel: ch,
                                    note,
                                    velocity: on_vel,
                                });
                            }
                        } else {
                            if let Some((start, on_vel)) = active.insert((ch, note), (tick, vel)) {
                                notes.push(MidiNote {
                                    start_tick: start,
                                    duration_ticks: tick.saturating_sub(start).max(1),
                                    channel: ch,
                                    note,
                                    velocity: on_vel,
                                });
                            }
                        }
                    }
                    0xA0 | 0xB0 | 0xE0 => {
                        pos = (pos + 2).min(trk_slice.len());
                    }
                    0xC0 | 0xD0 => {
                        pos = (pos + 1).min(trk_slice.len());
                    }
                    _ => break,
                }
            }
        }

        cursor = trk_end;
    }

    Ok(ParsedSmf {
        format,
        num_tracks,
        ticks_per_quarter,
        bpm,
        time_sig_num,
        time_sig_den,
        markers,
        notes,
        smf_end_offset: cursor,
    })
}

/// Configuration for a single accompaniment channel's voice and mix in Measure 1 (`SInt`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ChannelVoiceConfig {
    pub part: AccompanimentPart,
    /// Index into `PSR_E583_VOICES` (1-based `voice_no`).
    pub voice_no: u16,
    pub bank_msb: u8,
    pub bank_lsb: u8,
    pub program_change: u8,
    pub volume: u8,
    pub pan: u8,
    pub reverb: u8,
    pub chorus: u8,
    pub muted: bool,
}

impl ChannelVoiceConfig {
    pub fn default_for_part(part: AccompanimentPart) -> Self {
        match part {
            AccompanimentPart::Rhy1 => Self {
                part,
                voice_no: 359, // Standard Kit 1
                bank_msb: 127,
                bank_lsb: 0,
                program_change: 0,
                volume: 100,
                pan: 64,
                reverb: 40,
                chorus: 0,
                muted: false,
            },
            AccompanimentPart::Rhy2 => Self {
                part,
                voice_no: 353, // Real Drum Kit
                bank_msb: 127,
                bank_lsb: 0,
                program_change: 91, // PC# 92 (1-based) -> 91 (0-based)
                volume: 110,
                pan: 64,
                reverb: 40,
                chorus: 0,
                muted: false,
            },
            AccompanimentPart::Bass => Self {
                part,
                voice_no: 124, // Finger Bass
                bank_msb: 0,
                bank_lsb: 112,
                program_change: 33, // PC# 34 -> 33
                volume: 105,
                pan: 64,
                reverb: 20,
                chorus: 0,
                muted: false,
            },
            AccompanimentPart::Chd1 => Self {
                part,
                voice_no: 101, // Folk Guitar
                bank_msb: 0,
                bank_lsb: 112,
                program_change: 25, // PC# 26 -> 25
                volume: 90,
                pan: 48,
                reverb: 35,
                chorus: 20,
                muted: false,
            },
            AccompanimentPart::Chd2 => Self {
                part,
                voice_no: 28, // Cool! Suitcase Electric Piano
                bank_msb: 0,
                bank_lsb: 118,
                program_change: 4, // PC# 5 -> 4
                volume: 88,
                pan: 80,
                reverb: 40,
                chorus: 25,
                muted: false,
            },
            AccompanimentPart::Pad => Self {
                part,
                voice_no: 143, // Live! Strings
                bank_msb: 0,
                bank_lsb: 117,
                program_change: 49, // PC# 50 -> 49
                volume: 82,
                pan: 64,
                reverb: 55,
                chorus: 30,
                muted: false,
            },
            AccompanimentPart::Phr1 => Self {
                part,
                voice_no: 181, // Live! Brass
                bank_msb: 0,
                bank_lsb: 117,
                program_change: 62, // PC# 63 -> 62
                volume: 92,
                pan: 54,
                reverb: 45,
                chorus: 15,
                muted: false,
            },
            AccompanimentPart::Phr2 => Self {
                part,
                voice_no: 281, // Gemini Synth Lead
                bank_msb: 104,
                bank_lsb: 20,
                program_change: 90, // PC# 91 -> 90
                volume: 85,
                pan: 74,
                reverb: 50,
                chorus: 35,
                muted: false,
            },
        }
    }
}

/// Builds a complete SMF Format 0 byte array (`MThd` + `MTrk` ending in `FF 2F 00`)
/// with Measure 1 initialization (`SFF1` + `SInt`) and Measure 2+ sections.
pub fn build_sff1_smf0(
    style_name: &str,
    bpm: f32,
    time_sig_num: u8,
    time_sig_den: u8,
    ticks_per_quarter: u16,
    channel_configs: &[ChannelVoiceConfig],
    sections: &[(StyleSection, usize, Vec<MidiNote>)], // (Section, length_in_bars, notes relative to section start)
) -> Vec<u8> {
    let tpq = ticks_per_quarter.max(96) as u32;
    let num = time_sig_num.max(1) as u32;
    let den = time_sig_den.max(1) as u32;
    let ticks_per_bar = tpq * num * 4 / den;

    let mut events: Vec<RawSmfEvent> = Vec::new();

    // --- Measure 1 (Tick 0): Initialization ---
    let denom_pow = (den as f32).log2().round() as u8;
    events.push(RawSmfEvent::Meta {
        tick: 0,
        meta_type: 0x58, // Time Signature
        data: vec![time_sig_num.max(1), denom_pow, 24, 8],
    });

    let us_per_qn = (60_000_000.0 / bpm.clamp(30.0, 300.0)).round() as u32;
    events.push(RawSmfEvent::Meta {
        tick: 0,
        meta_type: 0x51, // Tempo
        data: vec![
            ((us_per_qn >> 16) & 0xFF) as u8,
            ((us_per_qn >> 8) & 0xFF) as u8,
            (us_per_qn & 0xFF) as u8,
        ],
    });

    // Meta Marker: "SFF1"
    events.push(RawSmfEvent::Meta {
        tick: 0,
        meta_type: 0x06,
        data: b"SFF1".to_vec(),
    });

    // Track/Style Name (FF 03)
    let clean_name = if style_name.trim().is_empty() {
        "CustomStyle"
    } else {
        style_name.trim()
    };
    events.push(RawSmfEvent::Meta {
        tick: 0,
        meta_type: 0x03,
        data: clean_name.as_bytes().iter().copied().take(32).collect(),
    });

    // Meta Marker: "SInt" (Style Initialization)
    events.push(RawSmfEvent::Meta {
        tick: 0,
        meta_type: 0x06,
        data: b"SInt".to_vec(),
    });

    // GM System ON: F0 7E 7F 09 01 F7
    events.push(RawSmfEvent::SysEx {
        tick: 0,
        data: vec![0x7E, 0x7F, 0x09, 0x01, 0xF7],
    });

    // XG System ON: F0 43 10 4C 00 00 7E 00 F7
    events.push(RawSmfEvent::SysEx {
        tick: 0,
        data: vec![0x43, 0x10, 0x4C, 0x00, 0x00, 0x7E, 0x00, 0xF7],
    });

    // Per-channel Bank Select, Program Change, Volume, Pan, Reverb, Chorus at tick 10 (inside Measure 1)
    for cfg in channel_configs {
        let ch = cfg.part.midi_channel();
        // Bank Select MSB (CC#0)
        events.push(RawSmfEvent::ControlChange {
            tick: 10,
            channel: ch,
            controller: 0,
            value: cfg.bank_msb & 0x7F,
        });
        // Bank Select LSB (CC#32)
        events.push(RawSmfEvent::ControlChange {
            tick: 10,
            channel: ch,
            controller: 32,
            value: cfg.bank_lsb & 0x7F,
        });
        // Program Change
        events.push(RawSmfEvent::ProgramChange {
            tick: 10,
            channel: ch,
            program: cfg.program_change & 0x7F,
        });
        // Volume (CC#7)
        events.push(RawSmfEvent::ControlChange {
            tick: 10,
            channel: ch,
            controller: 7,
            value: if cfg.muted { 0 } else { cfg.volume & 0x7F },
        });
        // Pan (CC#10)
        events.push(RawSmfEvent::ControlChange {
            tick: 10,
            channel: ch,
            controller: 10,
            value: cfg.pan & 0x7F,
        });
        // Reverb Send (CC#91)
        events.push(RawSmfEvent::ControlChange {
            tick: 10,
            channel: ch,
            controller: 91,
            value: cfg.reverb & 0x7F,
        });
        // Chorus Send (CC#93)
        events.push(RawSmfEvent::ControlChange {
            tick: 10,
            channel: ch,
            controller: 93,
            value: cfg.chorus & 0x7F,
        });
    }

    // --- Measure 2 Onwards: Style Sections ---
    let mut section_start_tick = ticks_per_bar; // Measure 2 begins after 1 bar of setup

    for (sec, bars, notes) in sections {
        let sec_bars = (*bars).max(1) as u32;
        let sec_len_ticks = sec_bars * ticks_per_bar;

        // Section Meta Marker (FF 06)
        events.push(RawSmfEvent::Meta {
            tick: section_start_tick,
            meta_type: 0x06,
            data: sec.as_marker_str().as_bytes().to_vec(),
        });

        for n in notes {
            if n.start_tick >= sec_len_ticks {
                continue;
            }
            let abs_start = section_start_tick + n.start_tick;
            // Clamp note-off so it never bleeds across the section seam
            let max_dur = sec_len_ticks.saturating_sub(n.start_tick).max(1);
            let dur = n.duration_ticks.min(max_dur).max(1);
            let abs_end = abs_start + dur;

            events.push(RawSmfEvent::NoteOn {
                tick: abs_start,
                channel: n.channel & 0x0F,
                note: n.note & 0x7F,
                velocity: n.velocity.clamp(1, 127),
            });
            events.push(RawSmfEvent::NoteOff {
                tick: abs_end,
                channel: n.channel & 0x0F,
                note: n.note & 0x7F,
                velocity: 0,
            });
        }

        section_start_tick += sec_len_ticks;
    }

    // End of Track Meta Event (FF 2F 00) at final bar boundary
    events.push(RawSmfEvent::Meta {
        tick: section_start_tick,
        meta_type: 0x2F,
        data: Vec::new(),
    });

    // Sort deterministically by (tick, sort_rank)
    events.sort_by_key(|e| (e.tick(), e.sort_rank()));

    // Serialize MTrk payload
    let mut trk_bytes = Vec::new();
    let mut last_tick = 0u32;
    for ev in events {
        let t = ev.tick();
        let delta = t.saturating_sub(last_tick);
        write_vlq(delta, &mut trk_bytes);
        last_tick = t;

        match ev {
            RawSmfEvent::Meta {
                meta_type, data, ..
            } => {
                trk_bytes.push(0xFF);
                trk_bytes.push(meta_type);
                write_vlq(data.len() as u32, &mut trk_bytes);
                trk_bytes.extend_from_slice(&data);
            }
            RawSmfEvent::SysEx { data, .. } => {
                trk_bytes.push(0xF0);
                write_vlq(data.len() as u32, &mut trk_bytes);
                trk_bytes.extend_from_slice(&data);
            }
            RawSmfEvent::ControlChange {
                channel,
                controller,
                value,
                ..
            } => {
                trk_bytes.push(0xB0 | (channel & 0x0F));
                trk_bytes.push(controller & 0x7F);
                trk_bytes.push(value & 0x7F);
            }
            RawSmfEvent::ProgramChange {
                channel, program, ..
            } => {
                trk_bytes.push(0xC0 | (channel & 0x0F));
                trk_bytes.push(program & 0x7F);
            }
            RawSmfEvent::NoteOff {
                channel,
                note,
                velocity,
                ..
            } => {
                trk_bytes.push(0x80 | (channel & 0x0F));
                trk_bytes.push(note & 0x7F);
                trk_bytes.push(velocity & 0x7F);
            }
            RawSmfEvent::NoteOn {
                channel,
                note,
                velocity,
                ..
            } => {
                trk_bytes.push(0x90 | (channel & 0x0F));
                trk_bytes.push(note & 0x7F);
                trk_bytes.push(velocity & 0x7F);
            }
        }
    }

    // Wrap in MThd (Format 0, 1 track) + MTrk
    let mut smf = Vec::with_capacity(22 + trk_bytes.len());
    smf.extend_from_slice(b"MThd");
    smf.extend_from_slice(&6u32.to_be_bytes());
    smf.extend_from_slice(&0u16.to_be_bytes()); // Format 0
    smf.extend_from_slice(&1u16.to_be_bytes()); // 1 Track
    smf.extend_from_slice(&ticks_per_quarter.max(96).to_be_bytes());

    smf.extend_from_slice(b"MTrk");
    smf.extend_from_slice(&(trk_bytes.len() as u32).to_be_bytes());
    smf.extend_from_slice(&trk_bytes);

    smf
}
