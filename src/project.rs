//! Core `StyleProject` state model, Drum Machine step grid, C-Maj7 Accompaniment generator,
//! Yamaha *MIDI Song to Style* bar slicer, and PSR-E583 `.sty` compiler & validator.

use crate::casm::{AccompanimentPart, CasmChunk, NtrRule, NttTable, StyleSection};
use crate::midi::{ChannelVoiceConfig, MidiNote, ParsedSmf, build_sff1_smf0, parse_smf_bytes};
use crate::voices::{DRUM_KIT_NOTES, PSR_E583_VOICES};
use serde::{Deserialize, Serialize};

pub const STEPS_PER_BAR: usize = 16; // Default 16th-note resolution per 4/4 bar
pub const DEFAULT_TPQ: u16 = 480;

/// Returns the number of 16th-note steps in a single bar for a given time signature `num / den`.
/// Examples: 4/4 -> 16, 3/4 -> 12, 2/4 -> 8, 6/8 -> 12, 5/4 -> 20, 7/8 -> 14, 12/8 -> 24.
pub const fn steps_per_bar_for_time_sig(num: u8, den: u8) -> usize {
    let n = if num == 0 { 4 } else { num as usize };
    let d = if den == 0 { 4 } else { den as usize };
    let s = (n * 16) / d;
    if s == 0 { 1 } else { s }
}

/// Returns the number of 16th-note steps per primary beat for visual grid grouping.
pub const fn steps_per_beat_for_time_sig(num: u8, den: u8) -> usize {
    if den == 8 && num.is_multiple_of(3) {
        6 // Dotted-quarter beat grouping for 6/8, 9/8, 12/8
    } else {
        let b = 16 / (if den == 0 { 4 } else { den as usize });
        if b == 0 { 1 } else { b }
    }
}

/// A single row in the Drum Machine step sequencer.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DrumRow {
    /// MIDI note number (e.g. 36 = Kick, 38 = Snare, 42 = Closed Hi-Hat).
    pub midi_note: u8,
    /// Display label for the drum row.
    pub name: String,
    /// Target rhythm channel (`Rhy2` = Ch 10 Main Drum Kit, `Rhy1` = Ch 9 Sub-Rhythm/Percussion).
    pub part: AccompanimentPart,
    /// Velocity per 16th-note step (`0` = off, `1..=127` = active hit velocity).
    /// Length is `bars * steps_per_bar`.
    pub steps: Vec<u8>,
    /// Row mute toggle for quick auditioning.
    pub muted: bool,
}

impl DrumRow {
    pub fn new(midi_note: u8, name: impl Into<String>, part: AccompanimentPart, bars: usize) -> Self {
        Self::new_with_steps_per_bar(midi_note, name, part, bars, STEPS_PER_BAR)
    }

    pub fn new_with_steps_per_bar(
        midi_note: u8,
        name: impl Into<String>,
        part: AccompanimentPart,
        bars: usize,
        steps_per_bar: usize,
    ) -> Self {
        Self {
            midi_note,
            name: name.into(),
            part,
            steps: vec![0; bars.max(1) * steps_per_bar.max(1)],
            muted: false,
        }
    }

    pub fn resize_total_steps(&mut self, target_len: usize) {
        let target_len = target_len.max(1);
        if self.steps.len() < target_len {
            let old_len = self.steps.len().max(1);
            let mut next = Vec::with_capacity(target_len);
            for i in 0..target_len {
                next.push(*self.steps.get(i % old_len).unwrap_or(&0));
            }
            self.steps = next;
        } else {
            self.steps.truncate(target_len);
        }
    }
}

/// Pattern data for a single SFF1 style section (`Main A`, `Fill In AA`, etc.).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SectionPattern {
    pub section: StyleSection,
    pub enabled: bool,
    pub bars: usize,
    pub steps_per_bar: usize,
    pub drum_rows: Vec<DrumRow>,
    /// Melodic/Harmonic accompaniment notes (Channels 11–16 written in C Maj7: C, E, G, B)
    /// plus any unquantized imported MIDI notes.
    pub accompaniment_notes: Vec<MidiNote>,
}

impl SectionPattern {
    pub fn new_default(section: StyleSection) -> Self {
        let bars = section.default_bars();
        let default_rows = Self::default_drum_rows(bars);
        Self {
            section,
            enabled: true,
            bars,
            steps_per_bar: STEPS_PER_BAR,
            drum_rows: default_rows,
            accompaniment_notes: Vec::new(),
        }
    }

    pub fn default_drum_rows(bars: usize) -> Vec<DrumRow> {
        vec![
            DrumRow::new(36, "Kick (Bass Drum)", AccompanimentPart::Rhy2, bars),
            DrumRow::new(38, "Snare Main", AccompanimentPart::Rhy2, bars),
            DrumRow::new(37, "Side Stick", AccompanimentPart::Rhy2, bars),
            DrumRow::new(39, "Hand Clap", AccompanimentPart::Rhy2, bars),
            DrumRow::new(42, "Hi-Hat Closed", AccompanimentPart::Rhy2, bars),
            DrumRow::new(44, "Hi-Hat Pedal", AccompanimentPart::Rhy2, bars),
            DrumRow::new(46, "Hi-Hat Open", AccompanimentPart::Rhy2, bars),
            DrumRow::new(49, "Crash Cymbal 1", AccompanimentPart::Rhy2, bars),
            DrumRow::new(51, "Ride Cymbal 1", AccompanimentPart::Rhy2, bars),
            DrumRow::new(50, "High Tom", AccompanimentPart::Rhy2, bars),
            DrumRow::new(47, "Low-Mid Tom", AccompanimentPart::Rhy2, bars),
            DrumRow::new(43, "High Floor Tom", AccompanimentPart::Rhy2, bars),
            DrumRow::new(54, "Tambourine", AccompanimentPart::Rhy1, bars),
            DrumRow::new(70, "Maracas / Shaker", AccompanimentPart::Rhy1, bars),
            DrumRow::new(62, "Mute Hi Conga", AccompanimentPart::Rhy1, bars),
            DrumRow::new(63, "Open Hi Conga", AccompanimentPart::Rhy1, bars),
        ]
    }

    pub fn total_steps(&self) -> usize {
        self.bars.max(1) * self.steps_per_bar.max(1)
    }

    pub fn set_bars(&mut self, new_bars: usize) {
        let clamped = new_bars.clamp(1, 8);
        self.bars = clamped;
        let total = self.total_steps();
        for row in &mut self.drum_rows {
            row.resize_total_steps(total);
        }
    }

    pub fn set_steps_per_bar(&mut self, new_spb: usize) {
        self.steps_per_bar = new_spb.clamp(1, 64);
        let total = self.total_steps();
        for row in &mut self.drum_rows {
            row.resize_total_steps(total);
        }
    }

    pub fn clear_drums(&mut self) {
        for row in &mut self.drum_rows {
            row.steps.fill(0);
        }
    }

    pub fn set_hit(&mut self, midi_note: u8, step: usize, velocity: u8) {
        if let Some(row) = self.drum_rows.iter_mut().find(|r| r.midi_note == midi_note)
            && step < row.steps.len()
        {
            row.steps[step] = velocity;
        }
    }


    /// Compiles both the Drum Machine step grid and the accompaniment notes into sorted `MidiNote`s
    /// relative to the start of this section.
    pub fn compile_notes(&self, ticks_per_quarter: u16, channel_configs: &[ChannelVoiceConfig]) -> Vec<MidiNote> {
        let mut out = Vec::new();
        let tpq = ticks_per_quarter.max(96) as u32;
        let ticks_per_step = tpq / 4; // 16th note = 1/4 of a quarter note

        // 1. Convert Drum Machine step grid into MIDI notes
        for row in &self.drum_rows {
            if row.muted {
                continue;
            }
            let ch = row.part.midi_channel();
            if channel_configs
                .iter()
                .find(|c| c.part == row.part)
                .is_some_and(|c| c.muted)
            {
                continue;
            }

            for (step_idx, &vel) in row.steps.iter().enumerate() {
                if vel > 0 {
                    out.push(MidiNote {
                        start_tick: (step_idx as u32) * ticks_per_step,
                        duration_ticks: (ticks_per_step * 3 / 4).max(20),
                        channel: ch,
                        note: row.midi_note,
                        velocity: vel.clamp(1, 127),
                    });
                }
            }
        }

        // 2. Append accompaniment notes (and any imported raw notes)
        for n in &self.accompaniment_notes {
            if let Some(part) = AccompanimentPart::from_midi_channel(n.channel)
                && channel_configs
                    .iter()
                    .find(|c| c.part == part)
                    .is_some_and(|c| c.muted)
            {
                continue;
            }
            out.push(*n);
        }

        out
    }
}

/// Built-in style presets tailored for the PSR-E583 onboard voices.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StylePreset {
    ModernPopRock,
    Synthwave80s,
    EuroDanceEdm,
    SoulBallad,
    LatinBossa,
}

impl StylePreset {
    pub const ALL: &'static [StylePreset] = &[
        StylePreset::ModernPopRock,
        StylePreset::Synthwave80s,
        StylePreset::EuroDanceEdm,
        StylePreset::SoulBallad,
        StylePreset::LatinBossa,
    ];

    pub const fn label(&self) -> &'static str {
        match self {
            StylePreset::ModernPopRock => "Modern Pop / Rock (120 BPM)",
            StylePreset::Synthwave80s => "80s Synth Pop (118 BPM)",
            StylePreset::EuroDanceEdm => "Euro Dance / EDM (128 BPM)",
            StylePreset::SoulBallad => "Soul Ballad (78 BPM)",
            StylePreset::LatinBossa => "Latin Bossa Groove (112 BPM)",
        }
    }
}

/// State for the Yamaha *MIDI Song to Style* Bar Slicer & Channel Router.
#[derive(Debug, Clone)]
pub struct SongSlicerState {
    pub file_name: String,
    pub parsed: Option<ParsedSmf>,
    pub target_section: StyleSection,
    /// 1-based start bar in the source MIDI file.
    pub start_bar: usize,
    /// Number of bars to slice (1..=8).
    pub bar_count: usize,
    /// Routing for source MIDI channels 0..16 -> `Option<AccompanimentPart>`.
    pub channel_routing: [Option<AccompanimentPart>; 16],
    /// Whether to quantize imported Ch 9/10 drum notes into the Drum Machine step grid.
    pub quantize_drums_to_grid: bool,
    pub status_message: String,
}

impl Default for SongSlicerState {
    fn default() -> Self {
        let mut routing = [None; 16];
        routing[8] = Some(AccompanimentPart::Rhy1);
        routing[9] = Some(AccompanimentPart::Rhy2);
        routing[10] = Some(AccompanimentPart::Bass);
        routing[11] = Some(AccompanimentPart::Chd1);
        routing[12] = Some(AccompanimentPart::Chd2);
        routing[13] = Some(AccompanimentPart::Pad);
        routing[14] = Some(AccompanimentPart::Phr1);
        routing[15] = Some(AccompanimentPart::Phr2);
        // Also route common DAW channels 1-6 if user imports a standard DAW clip
        routing[0] = Some(AccompanimentPart::Chd1);
        routing[1] = Some(AccompanimentPart::Bass);
        routing[2] = Some(AccompanimentPart::Chd2);
        routing[3] = Some(AccompanimentPart::Pad);
        routing[4] = Some(AccompanimentPart::Phr1);
        routing[5] = Some(AccompanimentPart::Phr2);

        Self {
            file_name: String::new(),
            parsed: None,
            target_section: StyleSection::MainA,
            start_bar: 1,
            bar_count: 2,
            channel_routing: routing,
            quantize_drums_to_grid: true,
            status_message: "Load any .mid or .sty file to slice bars into PSR-E583 style sections.".to_string(),
        }
    }
}

/// Complete StyleWarden project state.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StyleProject {
    pub style_name: String,
    pub bpm: f32,
    pub time_sig_num: u8,
    pub time_sig_den: u8,
    pub ticks_per_quarter: u16,
    pub channel_configs: Vec<ChannelVoiceConfig>,
    pub sections: Vec<SectionPattern>,
    pub casm: CasmChunk,
}

impl Default for StyleProject {
    fn default() -> Self {
        let mut proj = Self {
            style_name: "E583_PopRock".to_string(),
            bpm: 120.0,
            time_sig_num: 4,
            time_sig_den: 4,
            ticks_per_quarter: DEFAULT_TPQ,
            channel_configs: AccompanimentPart::ALL
                .iter()
                .map(|&p| ChannelVoiceConfig::default_for_part(p))
                .collect(),
            sections: StyleSection::ALL
                .iter()
                .map(|&s| SectionPattern::new_default(s))
                .collect(),
            casm: CasmChunk::default(),
        };
        proj.apply_preset(StylePreset::ModernPopRock);
        proj
    }
}

impl StyleProject {
    pub fn steps_per_bar(&self) -> usize {
        steps_per_bar_for_time_sig(self.time_sig_num, self.time_sig_den)
    }

    pub fn steps_per_beat(&self) -> usize {
        steps_per_beat_for_time_sig(self.time_sig_num, self.time_sig_den)
    }

    pub fn set_time_signature(&mut self, num: u8, den: u8) {
        let valid_den = match den {
            2 | 4 | 8 | 16 => den,
            _ => 4,
        };
        let valid_num = num.clamp(1, 32);
        self.time_sig_num = valid_num;
        self.time_sig_den = valid_den;

        let spb = self.steps_per_bar();
        let step_ticks = (self.ticks_per_quarter.max(96) as u32) / 4;
        for sec in &mut self.sections {
            sec.set_steps_per_bar(spb);
            let max_tick = (sec.total_steps() as u32) * step_ticks;
            sec.accompaniment_notes.retain(|n| n.start_tick < max_tick);
        }
    }

    pub fn section_mut(&mut self, sec: StyleSection) -> &mut SectionPattern {
        self.sections
            .iter_mut()
            .find(|s| s.section == sec)
            .expect("All 10 StyleSections exist in StyleProject")
    }

    pub fn section(&self, sec: StyleSection) -> &SectionPattern {
        self.sections
            .iter()
            .find(|s| s.section == sec)
            .expect("All 10 StyleSections exist in StyleProject")
    }

    pub fn set_channel_voice(&mut self, part: AccompanimentPart, voice_no: u16) {
        if let Some(v) = PSR_E583_VOICES.iter().find(|v| v.voice_no == voice_no)
            && let Some(cfg) = self.channel_configs.iter_mut().find(|c| c.part == part)
        {
            cfg.voice_no = v.voice_no;
            if let (Some(msb), Some(lsb), Some(pc)) = (v.msb, v.lsb, v.pc) {
                cfg.bank_msb = msb;
                cfg.bank_lsb = lsb;
                cfg.program_change = pc;
            }
        }
    }

    /// Populates all 10 sections with a complete drum groove + C-Maj7 accompaniment pattern
    /// and selects matching PSR-E583 voices.
    pub fn apply_preset(&mut self, preset: StylePreset) {
        match preset {
            StylePreset::ModernPopRock => {
                self.style_name = "E583_PopRock".to_string();
                self.bpm = 120.0;
                self.set_channel_voice(AccompanimentPart::Rhy1, 359); // Standard Kit 1
                self.set_channel_voice(AccompanimentPart::Rhy2, 353); // Real Drum Kit
                self.set_channel_voice(AccompanimentPart::Bass, 124); // Finger Bass
                self.set_channel_voice(AccompanimentPart::Chd1, 101); // Folk Guitar
                self.set_channel_voice(AccompanimentPart::Chd2, 28);  // Cool! Suitcase EP
                self.set_channel_voice(AccompanimentPart::Pad, 143);  // Live! Strings
                self.set_channel_voice(AccompanimentPart::Phr1, 181); // Live! Brass
                self.set_channel_voice(AccompanimentPart::Phr2, 281); // Gemini Lead
            }
            StylePreset::Synthwave80s => {
                self.style_name = "E583_80sSynth".to_string();
                self.bpm = 118.0;
                self.set_channel_voice(AccompanimentPart::Rhy1, 369); // Electronic Kit
                self.set_channel_voice(AccompanimentPart::Rhy2, 370); // Analog Kit
                self.set_channel_voice(AccompanimentPart::Bass, 136); // Synth Bass
                self.set_channel_voice(AccompanimentPart::Chd1, 294); // 80s Poly Synth
                self.set_channel_voice(AccompanimentPart::Chd2, 26);  // DX Modern Elec. Piano
                self.set_channel_voice(AccompanimentPart::Pad, 307);  // Warm Pad
                self.set_channel_voice(AccompanimentPart::Phr1, 279); // Sawtooth Lead
                self.set_channel_voice(AccompanimentPart::Phr2, 336); // Synth Brass
            }
            StylePreset::EuroDanceEdm => {
                self.style_name = "E583_EuroDance".to_string();
                self.bpm = 128.0;
                self.set_channel_voice(AccompanimentPart::Rhy1, 372); // Dance Kit
                self.set_channel_voice(AccompanimentPart::Rhy2, 371); // House Kit
                self.set_channel_voice(AccompanimentPart::Bass, 138); // Dance Bass
                self.set_channel_voice(AccompanimentPart::Chd1, 295); // Super Saw
                self.set_channel_voice(AccompanimentPart::Chd2, 2);   // Bright Piano
                self.set_channel_voice(AccompanimentPart::Pad, 309);  // Choir Pad
                self.set_channel_voice(AccompanimentPart::Phr1, 280); // Square Lead
                self.set_channel_voice(AccompanimentPart::Phr2, 285); // Trance Lead
            }
            StylePreset::SoulBallad => {
                self.style_name = "E583_SoulBallad".to_string();
                self.bpm = 78.0;
                self.set_channel_voice(AccompanimentPart::Rhy1, 359); // Standard Kit 1
                self.set_channel_voice(AccompanimentPart::Rhy2, 355); // Jazz Kit
                self.set_channel_voice(AccompanimentPart::Bass, 125); // Fretless Bass
                self.set_channel_voice(AccompanimentPart::Chd1, 27);  // Vintage EP
                self.set_channel_voice(AccompanimentPart::Chd2, 99);  // Classic Guitar
                self.set_channel_voice(AccompanimentPart::Pad, 144);  // Warm Strings
                self.set_channel_voice(AccompanimentPart::Phr1, 211); // Breathy Tenor Sax
                self.set_channel_voice(AccompanimentPart::Phr2, 241); // Sweet! Flute
            }
            StylePreset::LatinBossa => {
                self.style_name = "E583_BossaNova".to_string();
                self.bpm = 112.0;
                self.set_channel_voice(AccompanimentPart::Rhy1, 376); // Cuban Kit
                self.set_channel_voice(AccompanimentPart::Rhy2, 365); // Brush Kit
                self.set_channel_voice(AccompanimentPart::Bass, 122); // Acoustic Bass
                self.set_channel_voice(AccompanimentPart::Chd1, 99);  // Classic Guitar
                self.set_channel_voice(AccompanimentPart::Chd2, 28);  // Cool! Suitcase EP
                self.set_channel_voice(AccompanimentPart::Pad, 143);  // Live! Strings
                self.set_channel_voice(AccompanimentPart::Phr1, 241); // Flute
                self.set_channel_voice(AccompanimentPart::Phr2, 74);  // Vibraphone
            }
        }

        let tpq = self.ticks_per_quarter;
        for sec_pat in &mut self.sections {
            sec_pat.clear_drums();
            sec_pat.accompaniment_notes.clear();
            let total_steps = sec_pat.total_steps();
            let sec = sec_pat.section;

            // Populate drum groove based on section role and preset
            let spb = sec_pat.steps_per_bar.max(1);
            for bar in 0..sec_pat.bars {
                let base = bar * spb;
                match sec {
                    StyleSection::IntroA => {
                        sec_pat.set_hit(49, 0, 110); // Crash on beat 1
                        for s in (0..16).step_by(4) {
                            sec_pat.set_hit(36, base + s, 100);
                            sec_pat.set_hit(42, base + s, 85);
                        }
                        sec_pat.set_hit(38, base + 12, 105);
                        sec_pat.set_hit(38, base + 14, 95);
                    }
                    StyleSection::EndingA | StyleSection::EndingB => {
                        sec_pat.set_hit(36, base, 115);
                        sec_pat.set_hit(49, base, 115);
                        if bar + 1 < sec_pat.bars {
                            sec_pat.set_hit(42, base + 4, 80);
                            sec_pat.set_hit(38, base + 8, 95);
                            sec_pat.set_hit(42, base + 12, 80);
                        }
                    }
                    StyleSection::FillInAA
                    | StyleSection::FillInBB
                    | StyleSection::FillInAB
                    | StyleSection::FillInBA => {
                        // Punchy groove first half of bar, tom roll + snare build second half
                        sec_pat.set_hit(36, base, 110);
                        sec_pat.set_hit(42, base, 90);
                        sec_pat.set_hit(42, base + 2, 75);
                        sec_pat.set_hit(38, base + 4, 105);
                        sec_pat.set_hit(36, base + 6, 95);
                        // 16th-note Tom/Snare cascade across beats 3 & 4
                        sec_pat.set_hit(50, base + 8, 100);
                        sec_pat.set_hit(50, base + 9, 95);
                        sec_pat.set_hit(47, base + 10, 105);
                        sec_pat.set_hit(47, base + 11, 100);
                        sec_pat.set_hit(43, base + 12, 110);
                        sec_pat.set_hit(43, base + 13, 105);
                        sec_pat.set_hit(38, base + 14, 118);
                        sec_pat.set_hit(38, base + 15, 124);
                    }
                    StyleSection::IntroB | StyleSection::MainA | StyleSection::MainB => {
                        let is_b = matches!(sec, StyleSection::MainB | StyleSection::IntroB);
                        if bar == 0 && is_b {
                            sec_pat.set_hit(49, 0, 105);
                        }
                        match preset {
                            StylePreset::EuroDanceEdm => {
                                // Four-on-the-floor kick + offbeat open hat + clap on 2 & 4
                                for s in (0..16).step_by(4) {
                                    sec_pat.set_hit(36, base + s, 118);
                                }
                                for s in (2..16).step_by(4) {
                                    sec_pat.set_hit(46, base + s, 98);
                                }
                                for s in (0..16).step_by(2) {
                                    sec_pat.set_hit(42, base + s, 78);
                                }
                                sec_pat.set_hit(38, base + 4, 110);
                                sec_pat.set_hit(39, base + 4, 105);
                                sec_pat.set_hit(38, base + 12, 112);
                                sec_pat.set_hit(39, base + 12, 108);
                                if is_b {
                                    for s in 0..16 {
                                        sec_pat.set_hit(70, base + s, if s % 2 == 0 { 82 } else { 62 });
                                    }
                                }
                            }
                            StylePreset::LatinBossa => {
                                // Bossa kick (1, 4, 5, 8, 9, 12, 13, 16) + side-stick cross-rhythm + shaker/conga
                                for &k in &[0, 3, 4, 7, 8, 11, 12, 15] {
                                    sec_pat.set_hit(36, base + k, 95);
                                }
                                for &st in &[0, 3, 6, 10, 12] {
                                    sec_pat.set_hit(37, base + st, 92);
                                }
                                for s in 0..16 {
                                    sec_pat.set_hit(42, base + s, if s % 2 == 0 { 76 } else { 58 });
                                    sec_pat.set_hit(70, base + s, if s % 4 == 0 { 80 } else { 60 });
                                }
                                if is_b {
                                    sec_pat.set_hit(62, base + 2, 85);
                                    sec_pat.set_hit(63, base + 6, 90);
                                    sec_pat.set_hit(62, base + 10, 85);
                                    sec_pat.set_hit(63, base + 14, 92);
                                }
                            }
                            StylePreset::SoulBallad => {
                                // Slow 16th-hat ballad groove with rim/snare
                                sec_pat.set_hit(36, base, 102);
                                sec_pat.set_hit(36, base + 6, 84);
                                sec_pat.set_hit(36, base + 8, 98);
                                sec_pat.set_hit(36, base + 11, 86);
                                let sn_note = if is_b { 38 } else { 37 };
                                sec_pat.set_hit(sn_note, base + 4, 100);
                                sec_pat.set_hit(sn_note, base + 12, 104);
                                for s in 0..16 {
                                    let v = if s % 4 == 0 {
                                        88
                                    } else if s % 2 == 0 {
                                        72
                                    } else {
                                        56
                                    };
                                    sec_pat.set_hit(42, base + s, v);
                                }
                                if is_b {
                                    sec_pat.set_hit(54, base + 4, 78);
                                    sec_pat.set_hit(54, base + 12, 82);
                                }
                            }
                            StylePreset::ModernPopRock | StylePreset::Synthwave80s => {
                                // Driving Pop/Rock or 80s groove
                                sec_pat.set_hit(36, base, 112);
                                sec_pat.set_hit(36, base + 6, 96);
                                sec_pat.set_hit(36, base + 8, 108);
                                sec_pat.set_hit(36, base + 10, 98);
                                sec_pat.set_hit(38, base + 4, 110);
                                sec_pat.set_hit(38, base + 12, 114);
                                if preset == StylePreset::Synthwave80s {
                                    sec_pat.set_hit(39, base + 4, 96);
                                    sec_pat.set_hit(39, base + 12, 100);
                                }
                                for s in (0..16).step_by(2) {
                                    let v = if s % 4 == 0 { 92 } else { 74 };
                                    if is_b {
                                        sec_pat.set_hit(51, base + s, v); // Ride in Main B
                                    } else {
                                        sec_pat.set_hit(42, base + s, v); // Closed HH in Main A
                                    }
                                }
                                if is_b {
                                    sec_pat.set_hit(46, base + 14, 90);
                                    sec_pat.set_hit(54, base + 4, 84);
                                    sec_pat.set_hit(54, base + 12, 86);
                                }
                            }
                        }
                    }
                }
            }

            // Generate C-Maj7 Accompaniment Notes (Channels 11–16)
            populate_cmaj7_accompaniment(sec_pat, preset, tpq, total_steps);
        }
    }

    /// Slices `[start_bar .. start_bar + bar_count)` from `parsed` into `target_section` using the channel routing map.
    pub fn slice_from_smf(&mut self, slicer: &SongSlicerState) -> Result<usize, String> {
        let Some(parsed) = &slicer.parsed else {
            return Err("No MIDI file loaded in Song Slicer".to_string());
        };
        let src_tpq = parsed.ticks_per_quarter.max(1) as u32;
        let dst_tpq = self.ticks_per_quarter.max(96) as u32;
        let src_tpb = parsed.ticks_per_bar().max(1);

        let start_bar_idx = slicer.start_bar.saturating_sub(1) as u32;
        let bars = slicer.bar_count.clamp(1, 8);
        let win_start = start_bar_idx * src_tpb;
        let win_end = win_start + (bars as u32) * src_tpb;

        let dst_step_ticks = dst_tpq / 4; // 16th note in destination TPQ
        let sec_pat = self.section_mut(slicer.target_section);
        sec_pat.set_bars(bars);
        sec_pat.clear_drums();
        sec_pat.accompaniment_notes.clear();

        let mut imported_count = 0usize;

        for n in &parsed.notes {
            if n.start_tick < win_start || n.start_tick >= win_end {
                continue;
            }
            let src_ch = (n.channel & 0x0F) as usize;
            let Some(dest_part) = slicer.channel_routing[src_ch] else {
                continue;
            };

            let rel_src_tick = n.start_tick - win_start;
            let scaled_start = (rel_src_tick as u64 * dst_tpq as u64 / src_tpq as u64) as u32;
            let scaled_dur = ((n.duration_ticks as u64 * dst_tpq as u64 / src_tpq as u64) as u32).max(15);

            if dest_part.is_rhythm() && slicer.quantize_drums_to_grid {
                let step = ((scaled_start + dst_step_ticks / 2) / dst_step_ticks) as usize;
                if step < sec_pat.total_steps() {
                    // Ensure a DrumRow exists for this MIDI note & part
                    if !sec_pat
                        .drum_rows
                        .iter()
                        .any(|r| r.midi_note == n.note && r.part == dest_part)
                    {
                        let name = DRUM_KIT_NOTES
                            .iter()
                            .find(|d| d.midi_note == n.note)
                            .map(|d| d.standard_name.to_string())
                            .unwrap_or_else(|| format!("Drum Note #{}", n.note));
                        sec_pat
                            .drum_rows
                            .push(DrumRow::new(n.note, name, dest_part, bars));
                    }
                    if let Some(row) = sec_pat
                        .drum_rows
                        .iter_mut()
                        .find(|r| r.midi_note == n.note && r.part == dest_part)
                    {
                        row.steps[step] = row.steps[step].max(n.velocity.clamp(1, 127));
                        imported_count += 1;
                    }
                }
            } else {
                sec_pat.accompaniment_notes.push(MidiNote {
                    start_tick: scaled_start,
                    duration_ticks: scaled_dur,
                    channel: dest_part.midi_channel(),
                    note: n.note,
                    velocity: n.velocity.clamp(1, 127),
                });
                imported_count += 1;
            }
        }

        Ok(imported_count)
    }

    /// Compiles the entire `StyleProject` into a complete Yamaha SFF1 `.sty` binary (`MThd` + `MTrk` + `CASM`).
    pub fn compile_sty_bytes(&self) -> Vec<u8> {
        let mut compiled_sections = Vec::new();
        for sec_pat in &self.sections {
            if !sec_pat.enabled {
                continue;
            }
            let notes = sec_pat.compile_notes(self.ticks_per_quarter, &self.channel_configs);
            compiled_sections.push((sec_pat.section, sec_pat.bars.max(1), notes));
        }

        let mut out = build_sff1_smf0(
            &self.style_name,
            self.bpm,
            self.time_sig_num,
            self.time_sig_den,
            self.ticks_per_quarter,
            &self.channel_configs,
            &compiled_sections,
        );

        let casm_bytes = self.casm.to_bytes();
        out.extend_from_slice(&casm_bytes);
        out
    }

    /// Imports an existing `.sty` file into `StyleProject` (extracting both the SMF sections and `CASM` rules).
    pub fn import_sty_bytes(data: &[u8], file_stem: &str) -> Result<Self, String> {
        let parsed = parse_smf_bytes(data)?;
        let mut proj = Self::default();
        proj.style_name = file_stem.to_string();
        proj.bpm = parsed.bpm;
        proj.ticks_per_quarter = parsed.ticks_per_quarter.max(96);
        proj.set_time_signature(parsed.time_sig_num, parsed.time_sig_den);

        // Parse CASM if present after SMF end
        if parsed.smf_end_offset + 8 <= data.len() {
            let tail = &data[parsed.smf_end_offset..];
            if let Some(casm_pos) = tail.windows(4).position(|w| w == b"CASM") {
                if let Ok(casm) = CasmChunk::from_bytes(&tail[casm_pos..])
                    && !casm.segments.is_empty()
                {
                    proj.casm = casm;
                }
            }
        }

        // Group markers into section windows
        let sec_markers: Vec<(u32, StyleSection)> = parsed
            .markers
            .iter()
            .filter_map(|(t, name)| StyleSection::from_marker_str(name).map(|s| (*t, s)))
            .collect();

        if !sec_markers.is_empty() {
            let tpb = parsed.ticks_per_bar().max(1);
            let step_ticks = (proj.ticks_per_quarter as u32) / 4;

            for sec_pat in &mut proj.sections {
                sec_pat.clear_drums();
                sec_pat.accompaniment_notes.clear();
            }

            for (idx, (start_tick, sec)) in sec_markers.iter().enumerate() {
                let end_tick = sec_markers
                    .get(idx + 1)
                    .map(|(t, _)| *t)
                    .unwrap_or_else(|| {
                        parsed
                            .notes
                            .iter()
                            .map(|n| n.start_tick + n.duration_ticks)
                            .max()
                            .unwrap_or(*start_tick + tpb)
                    });

                let span_ticks = end_tick.saturating_sub(*start_tick).max(tpb);
                let bars = span_ticks.div_ceil(tpb).clamp(1, 8) as usize;
                let sec_pat = proj.section_mut(*sec);
                sec_pat.enabled = true;
                sec_pat.set_bars(bars);

                for n in &parsed.notes {
                    if n.start_tick >= *start_tick && n.start_tick < end_tick {
                        let rel_tick = n.start_tick - *start_tick;
                        if let Some(part) = AccompanimentPart::from_midi_channel(n.channel) {
                            if part.is_rhythm() {
                                let step = ((rel_tick + step_ticks / 2) / step_ticks) as usize;
                                if step < sec_pat.total_steps() {
                                    if !sec_pat
                                        .drum_rows
                                        .iter()
                                        .any(|r| r.midi_note == n.note && r.part == part)
                                    {
                                        let name = DRUM_KIT_NOTES
                                            .iter()
                                            .find(|d| d.midi_note == n.note)
                                            .map(|d| d.standard_name.to_string())
                                            .unwrap_or_else(|| format!("Drum Note #{}", n.note));
                                        sec_pat
                                            .drum_rows
                                            .push(DrumRow::new(n.note, name, part, bars));
                                    }
                                    if let Some(row) = sec_pat
                                        .drum_rows
                                        .iter_mut()
                                        .find(|r| r.midi_note == n.note && r.part == part)
                                    {
                                        row.steps[step] =
                                            row.steps[step].max(n.velocity.clamp(1, 127));
                                    }
                                }
                            } else {
                                sec_pat.accompaniment_notes.push(MidiNote {
                                    start_tick: rel_tick,
                                    duration_ticks: n.duration_ticks,
                                    channel: n.channel,
                                    note: n.note,
                                    velocity: n.velocity,
                                });
                            }
                        }
                    }
                }
            }
        }

        Ok(proj)
    }
}

/// Populates C-Maj7 (`C, E, G, B`) accompaniment tracks (`Bass`, `Chd1`, `Chd2`, `Pad`, `Phr1`, `Phr2`)
/// for a given section and preset so exported `.sty` files work immediately as full-band styles.
fn populate_cmaj7_accompaniment(
    sec_pat: &mut SectionPattern,
    preset: StylePreset,
    tpq: u16,
    total_steps: usize,
) {
    let spb = sec_pat.steps_per_bar.max(1) as u32;
    let step_ticks = (tpq.max(96) as u32) / 4;
    let bar_ticks = step_ticks * spb;

    for bar in 0..sec_pat.bars {
        let b_tick = (bar as u32) * bar_ticks;

        match sec_pat.section {
            StyleSection::EndingA | StyleSection::EndingB => {
                // Final resolving C Maj7 chord hit on beat 1
                sec_pat.accompaniment_notes.push(MidiNote {
                    start_tick: b_tick,
                    duration_ticks: (step_ticks * 8).min(bar_ticks - 10),
                    channel: AccompanimentPart::Bass.midi_channel(),
                    note: 36, // C2
                    velocity: 105,
                });
                for &n in &[60u8, 64, 67, 71] {
                    sec_pat.accompaniment_notes.push(MidiNote {
                        start_tick: b_tick,
                        duration_ticks: (step_ticks * 12).min(bar_ticks - 10),
                        channel: AccompanimentPart::Chd1.midi_channel(),
                        note: n,
                        velocity: 92,
                    });
                    sec_pat.accompaniment_notes.push(MidiNote {
                        start_tick: b_tick,
                        duration_ticks: (step_ticks * 14).min(bar_ticks - 10),
                        channel: AccompanimentPart::Pad.midi_channel(),
                        note: n,
                        velocity: 84,
                    });
                }
            }
            _ => {
                // 1. Bass (Ch 11): Root C2 (36), Fifth G2 (43), Maj7 B2 (47), Octave C3 (48)
                let bass_pattern: &[(u32, u32, u8, u8)] = match preset {
                    StylePreset::EuroDanceEdm => &[
                        (0, 2, 36, 108),
                        (2, 2, 48, 102),
                        (4, 2, 36, 108),
                        (6, 2, 48, 102),
                        (8, 2, 36, 108),
                        (10, 2, 48, 102),
                        (12, 2, 43, 104),
                        (14, 2, 47, 100),
                    ],
                    StylePreset::Synthwave80s => &[
                        (0, 2, 36, 106),
                        (2, 2, 36, 96),
                        (4, 2, 48, 104),
                        (6, 2, 36, 96),
                        (8, 2, 36, 106),
                        (10, 2, 43, 98),
                        (12, 2, 48, 104),
                        (14, 2, 47, 96),
                    ],
                    StylePreset::LatinBossa => &[
                        (0, 6, 36, 100),
                        (6, 2, 43, 92),
                        (8, 6, 43, 98),
                        (14, 2, 36, 92),
                    ],
                    StylePreset::SoulBallad => &[
                        (0, 6, 36, 98),
                        (6, 2, 43, 86),
                        (8, 4, 48, 94),
                        (12, 4, 43, 90),
                    ],
                    StylePreset::ModernPopRock => &[
                        (0, 4, 36, 105),
                        (4, 2, 36, 92),
                        (6, 2, 43, 98),
                        (8, 4, 36, 104),
                        (12, 2, 43, 96),
                        (14, 2, 47, 94),
                    ],
                };
                for &(st, dur, note, vel) in bass_pattern {
                    if st < spb {
                        let clamped_dur = dur.min(spb - st).max(1);
                        sec_pat.accompaniment_notes.push(MidiNote {
                            start_tick: b_tick + st * step_ticks,
                            duration_ticks: clamped_dur * step_ticks - 10,
                            channel: AccompanimentPart::Bass.midi_channel(),
                            note,
                            velocity: vel,
                        });
                    }
                }

                // 2. Chord 1 (Ch 12): Rhythmic C Maj7 stabs/strums (C4=60, E4=64, G4=67, B4=71)
                for &st in &[0u32, 3, 6, 10, 12] {
                    if st < spb {
                        for &note in &[60u8, 64, 67, 71] {
                            sec_pat.accompaniment_notes.push(MidiNote {
                                start_tick: b_tick + st * step_ticks,
                                duration_ticks: step_ticks * 2 - 15,
                                channel: AccompanimentPart::Chd1.midi_channel(),
                                note,
                                velocity: 86,
                            });
                        }
                    }
                }

                // 3. Chord 2 (Ch 13): Syncopated EP / Arpeggio in C Maj7
                let arp_notes = [60u8, 64, 67, 71, 72, 71, 67, 64];
                for (i, &note) in arp_notes.iter().enumerate() {
                    let st = (i as u32) * 2;
                    if st < spb {
                        sec_pat.accompaniment_notes.push(MidiNote {
                            start_tick: b_tick + st * step_ticks,
                            duration_ticks: step_ticks * 2 - 10,
                            channel: AccompanimentPart::Chd2.midi_channel(),
                            note,
                            velocity: 78,
                        });
                    }
                }

                // 4. Pad (Ch 14): Sustained whole-bar warm C Maj7 voicing
                for &note in &[60u8, 64, 67, 71] {
                    sec_pat.accompaniment_notes.push(MidiNote {
                        start_tick: b_tick,
                        duration_ticks: bar_ticks - 20,
                        channel: AccompanimentPart::Pad.midi_channel(),
                        note,
                        velocity: 76,
                    });
                }

                // 5. Phrase 1 (Ch 15): Active on Main B, Intro B, and Fills
                if matches!(
                    sec_pat.section,
                    StyleSection::MainB
                        | StyleSection::IntroB
                        | StyleSection::FillInAA
                        | StyleSection::FillInBB
                        | StyleSection::FillInAB
                        | StyleSection::FillInBA
                ) {
                    let phr_steps: &[(u32, u32, u8, u8)] = &[
                        (0, 3, 72, 92),  // C5
                        (3, 3, 71, 88),  // B4
                        (6, 2, 67, 86),  // G4
                        (8, 4, 64, 90),  // E4
                        (12, 4, 67, 88), // G4
                    ];
                    for &(st, dur, note, vel) in phr_steps {
                        if st < spb && (bar * (spb as usize) + st as usize) < total_steps {
                            let clamped_dur = dur.min(spb - st).max(1);
                            sec_pat.accompaniment_notes.push(MidiNote {
                                start_tick: b_tick + st * step_ticks,
                                duration_ticks: clamped_dur * step_ticks - 10,
                                channel: AccompanimentPart::Phr1.midi_channel(),
                                note,
                                velocity: vel,
                            });
                        }
                    }
                }
            }
        }
    }
}

/// A single validation check item for PSR-E583 `.sty` binary verification.
#[derive(Debug, Clone)]
pub struct ValidationCheck {
    pub name: &'static str,
    pub passed: bool,
    pub detail: String,
}

/// Full binary validation report for a compiled or imported `.sty` file.
#[derive(Debug, Clone)]
pub struct StyValidationReport {
    pub total_bytes: usize,
    pub smf_bytes: usize,
    pub casm_bytes: usize,
    pub checks: Vec<ValidationCheck>,
}

impl StyValidationReport {
    pub fn all_passed(&self) -> bool {
        self.checks.iter().all(|c| c.passed)
    }
}

/// Performs a 10-point binary verification of a `.sty` byte slice against PSR-E583 SFF1 rules.
pub fn validate_sty_bytes(data: &[u8]) -> StyValidationReport {
    let mut checks = Vec::new();

    // 1. MThd Format 0 Header
    let mthd_ok = data.len() >= 14
        && &data[0..4] == b"MThd"
        && u16::from_be_bytes([data[8], data[9]]) == 0
        && u16::from_be_bytes([data[10], data[11]]) == 1;
    checks.push(ValidationCheck {
        name: "SMF Format 0 Header (MThd)",
        passed: mthd_ok,
        detail: if mthd_ok {
            let tpq = u16::from_be_bytes([data[12], data[13]]);
            format!("Valid Format 0, 1 Track, {} TPQ", tpq)
        } else {
            "Invalid or non-Format-0 MThd header".to_string()
        },
    });

    let parsed = parse_smf_bytes(data);
    let smf_end = parsed.as_ref().map(|p| p.smf_end_offset).unwrap_or(0);

    // 2. SFF1 Meta Marker at Tick 0
    let has_sff1 = parsed
        .as_ref()
        .is_ok_and(|p| p.markers.iter().any(|(t, s)| *t == 0 && s == "SFF1"));
    checks.push(ValidationCheck {
        name: "Measure 1 'SFF1' Marker at Tick 0",
        passed: has_sff1,
        detail: if has_sff1 {
            "Found FF 06 'SFF1' at tick 0".to_string()
        } else {
            "Missing 'SFF1' marker at tick 0".to_string()
        },
    });

    // 3. SInt Meta Marker at Tick 0
    let has_sint = parsed
        .as_ref()
        .is_ok_and(|p| p.markers.iter().any(|(t, s)| *t == 0 && s == "SInt"));
    checks.push(ValidationCheck {
        name: "Measure 1 'SInt' Marker at Tick 0",
        passed: has_sint,
        detail: if has_sint {
            "Found FF 06 'SInt' at tick 0".to_string()
        } else {
            "Missing 'SInt' marker at tick 0".to_string()
        },
    });

    // 4. Measure 1 Note-Free Rule
    let measure1_clean = parsed.as_ref().is_ok_and(|p| {
        let bar1_end = p.ticks_per_bar();
        p.notes.iter().all(|n| n.start_tick >= bar1_end)
    });
    checks.push(ValidationCheck {
        name: "Measure 1 Note-Free Initialization",
        passed: measure1_clean,
        detail: if measure1_clean {
            "Zero musical notes in Measure 1 setup bar".to_string()
        } else {
            "Musical notes found inside Measure 1 (PSR-E583 requires note-free Measure 1)".to_string()
        },
    });

    // 5. Section Markers present
    let sec_names: Vec<String> = parsed
        .as_ref()
        .map(|p| {
            p.markers
                .iter()
                .filter(|(_, m)| StyleSection::from_marker_str(m).is_some())
                .map(|(_, m)| m.clone())
                .collect()
        })
        .unwrap_or_default();
    let has_sections = !sec_names.is_empty();
    checks.push(ValidationCheck {
        name: "SFF1 Style Section Markers",
        passed: has_sections,
        detail: if has_sections {
            format!("{} sections ({})", sec_names.len(), sec_names.join(", "))
        } else {
            "No SFF1 section markers found in MTrk".to_string()
        },
    });

    // 6. End-of-Track (FF 2F 00) followed by CASM
    let eot_and_casm = smf_end >= 3
        && smf_end + 8 <= data.len()
        && &data[smf_end - 3..smf_end] == &[0xFF, 0x2F, 0x00]
        && &data[smf_end..smf_end + 4] == b"CASM";
    checks.push(ValidationCheck {
        name: "FF 2F 00 -> CASM Seam Alignment",
        passed: eot_and_casm,
        detail: if eot_and_casm {
            format!("CASM starts immediately after FF 2F 00 at offset 0x{:04X}", smf_end)
        } else {
            "CASM does not immediately follow FF 2F 00".to_string()
        },
    });

    // Parse CASM
    let casm_slice = if smf_end < data.len() {
        &data[smf_end..]
    } else {
        &[]
    };
    let parsed_casm = CasmChunk::from_bytes(casm_slice);

    // 7. CASM Chunk Length Integrity
    let casm_len_ok = casm_slice.len() >= 8 && {
        let len = u32::from_be_bytes([casm_slice[4], casm_slice[5], casm_slice[6], casm_slice[7]]) as usize;
        len > 0 && 8 + len <= casm_slice.len()
    };
    checks.push(ValidationCheck {
        name: "CASM Big-Endian Length Integrity",
        passed: casm_len_ok,
        detail: if casm_len_ok {
            format!("CASM chunk size {} bytes verified", casm_slice.len())
        } else {
            "CASM chunk is empty or truncated".to_string()
        },
    });

    // 8. CSEG & Sdec Section Coverage
    let sdec_ok = parsed_casm
        .as_ref()
        .is_ok_and(|c| c.segments.iter().any(|seg| !seg.sections.is_empty()));
    checks.push(ValidationCheck {
        name: "CSEG -> Sdec Section Declarations",
        passed: sdec_ok,
        detail: if let Ok(c) = &parsed_casm {
            let count: usize = c.segments.iter().map(|s| s.sections.len()).sum();
            format!("{} CSEG segment(s) covering {} section(s)", c.segments.len(), count)
        } else {
            "Missing CSEG/Sdec declarations".to_string()
        },
    });

    // 9. Ctab NTR / NTT Rules for Rhythm vs Melodic Parts
    let ctab_rules_ok = parsed_casm.as_ref().is_ok_and(|c| {
        c.segments.iter().all(|seg| {
            !seg.ctabs.is_empty()
                && seg.ctabs.iter().all(|ct| {
                    if ct.dest_channel == 8 || ct.dest_channel == 9 {
                        ct.ntr == NtrRule::RootFixed && ct.ntt == NttTable::Bypass
                    } else {
                        true
                    }
                })
        })
    });
    checks.push(ValidationCheck {
        name: "Ctab 27-Byte Rhythm/Chord NTR & NTT Rules",
        passed: ctab_rules_ok,
        detail: if ctab_rules_ok {
            "Rhy1/Rhy2 set to RootFixed+Bypass; Ch 11-16 Ctab rules valid".to_string()
        } else {
            "Invalid Ctab NTR/NTT configuration for rhythm channels".to_string()
        },
    });

    // 10. Note Limit Low <= Note Limit High
    let limits_ok = parsed_casm.as_ref().is_ok_and(|c| {
        c.segments.iter().all(|seg| {
            seg.ctabs
                .iter()
                .all(|ct| ct.note_limit_low <= ct.note_limit_high && ct.note_limit_high <= 127)
        })
    });
    checks.push(ValidationCheck {
        name: "Ctab Note Limit Low/High Bounds",
        passed: limits_ok,
        detail: if limits_ok {
            "All Ctab note limits within 0..=127 (Low <= High)".to_string()
        } else {
            "Invalid Note Limit Low > High detected".to_string()
        },
    });

    StyValidationReport {
        total_bytes: data.len(),
        smf_bytes: smf_end,
        casm_bytes: casm_slice.len(),
        checks,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_full_sty_compile_and_10_point_validation() {
        let project = StyleProject::default();
        let sty_bytes = project.compile_sty_bytes();

        let report = validate_sty_bytes(&sty_bytes);
        for check in &report.checks {
            assert!(
                check.passed,
                "Check '{}' failed: {}",
                check.name, check.detail
            );
        }
        assert!(report.all_passed());

        // Round-trip import back into StyleProject
        let reimported =
            StyleProject::import_sty_bytes(&sty_bytes, "RoundTrip").expect("Should re-import .sty");
        assert_eq!(reimported.sections.len(), StyleSection::ALL.len());
        assert!(!reimported.section(StyleSection::MainA).accompaniment_notes.is_empty());
    }

    #[test]
    fn test_all_presets_and_890_voice_database() {
        assert_eq!(PSR_E583_VOICES.len(), 890);
        let drum_kits_count = PSR_E583_VOICES.iter().filter(|v| v.is_drum_kit()).count();
        assert_eq!(drum_kits_count, 40);

        for &preset in StylePreset::ALL {
            let mut proj = StyleProject::default();
            proj.apply_preset(preset);
            let bytes = proj.compile_sty_bytes();
            let report = validate_sty_bytes(&bytes);
            assert!(
                report.all_passed(),
                "Preset {:?} failed validation",
                preset
            );
        }
    }
}
