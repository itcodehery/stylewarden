//! CASM (Chord Assignment / Style Model) binary parser, inspector, and SFF1 generator.
//!
//! Hierarchy:
//! ```text
//! CASM (4-byte tag + u32 BE length)
//!  └── CSEG (Chord Segment - one or more per file)
//!       ├── Sdec (Section Declaration: ASCII comma-separated string, e.g. "Main A,Main B")
//!       ├── Ctab (27-byte SFF1 Channel Table payload for each channel 9..=16)
//!       └── Cntt (Optional 2-byte Note Transposition Table extension per channel)
//! ```

use serde::{Deserialize, Serialize};

/// Core SFF1 style sections supported by the Yamaha PSR-E583 (2-Main variation architecture).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum StyleSection {
    IntroA,
    IntroB,
    MainA,
    MainB,
    FillInAA,
    FillInBB,
    FillInAB,
    FillInBA,
    EndingA,
    EndingB,
}

impl StyleSection {
    pub const ALL: &'static [StyleSection] = &[
        StyleSection::IntroA,
        StyleSection::IntroB,
        StyleSection::MainA,
        StyleSection::MainB,
        StyleSection::FillInAA,
        StyleSection::FillInBB,
        StyleSection::FillInAB,
        StyleSection::FillInBA,
        StyleSection::EndingA,
        StyleSection::EndingB,
    ];

    /// Exact ASCII marker string written into SMF Meta Event `FF 06` and CASM `Sdec`.
    pub const fn as_marker_str(&self) -> &'static str {
        match self {
            StyleSection::IntroA => "Intro A",
            StyleSection::IntroB => "Intro B",
            StyleSection::MainA => "Main A",
            StyleSection::MainB => "Main B",
            StyleSection::FillInAA => "Fill In AA",
            StyleSection::FillInBB => "Fill In BB",
            StyleSection::FillInAB => "Fill In AB",
            StyleSection::FillInBA => "Fill In BA",
            StyleSection::EndingA => "Ending A",
            StyleSection::EndingB => "Ending B",
        }
    }

    pub const fn default_bars(&self) -> usize {
        match self {
            StyleSection::IntroA => 1,
            StyleSection::IntroB => 2,
            StyleSection::MainA => 2,
            StyleSection::MainB => 2,
            StyleSection::FillInAA => 1,
            StyleSection::FillInBB => 1,
            StyleSection::FillInAB => 1,
            StyleSection::FillInBA => 1,
            StyleSection::EndingA => 1,
            StyleSection::EndingB => 2,
        }
    }

    pub fn from_marker_str(s: &str) -> Option<Self> {
        match s.trim() {
            "Intro A" | "INTRO A" => Some(StyleSection::IntroA),
            "Intro B" | "INTRO B" => Some(StyleSection::IntroB),
            "Main A" | "MAIN A" => Some(StyleSection::MainA),
            "Main B" | "MAIN B" => Some(StyleSection::MainB),
            "Fill In AA" | "FILL IN AA" => Some(StyleSection::FillInAA),
            "Fill In BB" | "FILL IN BB" => Some(StyleSection::FillInBB),
            "Fill In AB" | "FILL IN AB" => Some(StyleSection::FillInAB),
            "Fill In BA" | "FILL IN BA" | "Break" => Some(StyleSection::FillInBA),
            "Ending A" | "ENDING A" => Some(StyleSection::EndingA),
            "Ending B" | "ENDING B" => Some(StyleSection::EndingB),
            _ => None,
        }
    }
}

/// Accompaniment part destination on the PSR-E583 (Channels 9–16 / 0-indexed 8–15).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum AccompanimentPart {
    /// Channel 9 (0x08): Sub-Rhythm / Percussion
    Rhy1,
    /// Channel 10 (0x09): Main Drum Kit
    Rhy2,
    /// Channel 11 (0x0A): Bass
    Bass,
    /// Channel 12 (0x0B): Chord 1
    Chd1,
    /// Channel 13 (0x0C): Chord 2
    Chd2,
    /// Channel 14 (0x0D): Pad
    Pad,
    /// Channel 15 (0x0E): Phrase 1
    Phr1,
    /// Channel 16 (0x0F): Phrase 2
    Phr2,
}

impl AccompanimentPart {
    pub const ALL: &'static [AccompanimentPart] = &[
        AccompanimentPart::Rhy1,
        AccompanimentPart::Rhy2,
        AccompanimentPart::Bass,
        AccompanimentPart::Chd1,
        AccompanimentPart::Chd2,
        AccompanimentPart::Pad,
        AccompanimentPart::Phr1,
        AccompanimentPart::Phr2,
    ];

    /// 0-indexed MIDI channel (`0x08`..=`0x0F`).
    pub const fn midi_channel(&self) -> u8 {
        match self {
            AccompanimentPart::Rhy1 => 8,
            AccompanimentPart::Rhy2 => 9,
            AccompanimentPart::Bass => 10,
            AccompanimentPart::Chd1 => 11,
            AccompanimentPart::Chd2 => 12,
            AccompanimentPart::Pad => 13,
            AccompanimentPart::Phr1 => 14,
            AccompanimentPart::Phr2 => 15,
        }
    }

    pub const fn short_label(&self) -> &'static str {
        match self {
            AccompanimentPart::Rhy1 => "Rhy1 (Ch 9)",
            AccompanimentPart::Rhy2 => "Rhy2 (Ch 10)",
            AccompanimentPart::Bass => "Bass (Ch 11)",
            AccompanimentPart::Chd1 => "Chd1 (Ch 12)",
            AccompanimentPart::Chd2 => "Chd2 (Ch 13)",
            AccompanimentPart::Pad => "Pad (Ch 14)",
            AccompanimentPart::Phr1 => "Phr1 (Ch 15)",
            AccompanimentPart::Phr2 => "Phr2 (Ch 16)",
        }
    }

    pub const fn default_track_name(&self) -> &'static str {
        match self {
            AccompanimentPart::Rhy1 => "Rhythm1 ",
            AccompanimentPart::Rhy2 => "Rhythm2 ",
            AccompanimentPart::Bass => "Bass    ",
            AccompanimentPart::Chd1 => "Chord1  ",
            AccompanimentPart::Chd2 => "Chord2  ",
            AccompanimentPart::Pad => "Pad     ",
            AccompanimentPart::Phr1 => "Phrase1 ",
            AccompanimentPart::Phr2 => "Phrase2 ",
        }
    }

    pub const fn is_rhythm(&self) -> bool {
        matches!(self, AccompanimentPart::Rhy1 | AccompanimentPart::Rhy2)
    }

    pub fn from_midi_channel(ch: u8) -> Option<Self> {
        match ch & 0x0F {
            8 => Some(AccompanimentPart::Rhy1),
            9 => Some(AccompanimentPart::Rhy2),
            10 => Some(AccompanimentPart::Bass),
            11 => Some(AccompanimentPart::Chd1),
            12 => Some(AccompanimentPart::Chd2),
            13 => Some(AccompanimentPart::Pad),
            14 => Some(AccompanimentPart::Phr1),
            15 => Some(AccompanimentPart::Phr2),
            _ => None,
        }
    }
}

/// NTR (Note Transposition Rule) byte at offset 20 of Ctab payload.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum NtrRule {
    /// `0x00`: Transposes root relative to the chord played (Bass, Chords, Pads, Phrases).
    RootTransposition = 0,
    /// `0x01`: Keeps root fixed (used for Drums/Percussion or fixed-key intros).
    RootFixed = 1,
}

impl NtrRule {
    pub const fn label(&self) -> &'static str {
        match self {
            NtrRule::RootTransposition => "Root Trans (0x00)",
            NtrRule::RootFixed => "Root Fixed (0x01)",
        }
    }

    pub const fn from_byte(b: u8) -> Self {
        match b {
            0 => NtrRule::RootTransposition,
            _ => NtrRule::RootFixed,
        }
    }
}

/// NTT (Note Transposition Table) byte at offset 21 of Ctab payload.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum NttTable {
    /// `0x00`: Bypass transposition (used with RootFixed for Drums/Percussion).
    Bypass = 0,
    /// `0x01`: Parallel / Melody transposition (Phrases, Melody lines).
    Melody = 1,
    /// `0x02`: Chordal harmony transposition (Chord 1, Chord 2, Pad).
    Chord = 2,
    /// `0x03`: Bass transposition (Bass channel).
    Bass = 3,
    /// `0x04`: Melodic Minor scale adjustment.
    MelodicMinor = 4,
    /// `0x05`: Harmonic Minor scale adjustment.
    HarmonicMinor = 5,
}

impl NttTable {
    pub const ALL: &'static [NttTable] = &[
        NttTable::Bypass,
        NttTable::Melody,
        NttTable::Chord,
        NttTable::Bass,
        NttTable::MelodicMinor,
        NttTable::HarmonicMinor,
    ];

    pub const fn label(&self) -> &'static str {
        match self {
            NttTable::Bypass => "Bypass (0x00)",
            NttTable::Melody => "Melody (0x01)",
            NttTable::Chord => "Chord (0x02)",
            NttTable::Bass => "Bass (0x03)",
            NttTable::MelodicMinor => "Melodic Minor (0x04)",
            NttTable::HarmonicMinor => "Harmonic Minor (0x05)",
        }
    }

    pub const fn from_byte(b: u8) -> Self {
        match b & 0x7F {
            0 => NttTable::Bypass,
            1 => NttTable::Melody,
            2 => NttTable::Chord,
            3 => NttTable::Bass,
            4 => NttTable::MelodicMinor,
            5 => NttTable::HarmonicMinor,
            _ => NttTable::Melody,
        }
    }
}

/// RTR (Retrigger Rule) byte at offset 25 of Ctab payload.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum RetriggerRule {
    Stop = 0,
    PitchShift = 1,
    PitchShiftToRoot = 2,
    Retrigger = 3,
    RetriggerToRoot = 4,
    NoteGenerator = 5,
}

impl RetriggerRule {
    pub const ALL: &'static [RetriggerRule] = &[
        RetriggerRule::Stop,
        RetriggerRule::PitchShift,
        RetriggerRule::PitchShiftToRoot,
        RetriggerRule::Retrigger,
        RetriggerRule::RetriggerToRoot,
        RetriggerRule::NoteGenerator,
    ];

    pub const fn label(&self) -> &'static str {
        match self {
            RetriggerRule::Stop => "Stop (0x00)",
            RetriggerRule::PitchShift => "Pitch Shift (0x01)",
            RetriggerRule::PitchShiftToRoot => "Pitch Shift to Root (0x02)",
            RetriggerRule::Retrigger => "Retrigger (0x03)",
            RetriggerRule::RetriggerToRoot => "Retrigger to Root (0x04)",
            RetriggerRule::NoteGenerator => "Note Generator (0x05)",
        }
    }

    pub const fn from_byte(b: u8) -> Self {
        match b {
            0 => RetriggerRule::Stop,
            1 => RetriggerRule::PitchShift,
            2 => RetriggerRule::PitchShiftToRoot,
            3 => RetriggerRule::Retrigger,
            4 => RetriggerRule::RetriggerToRoot,
            _ => RetriggerRule::NoteGenerator,
        }
    }
}

/// Represents a single 27-byte `Ctab` (Channel Table) block in SFF1 format.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CtabEntry {
    /// Source MIDI Channel (0x00..=0x0F).
    pub source_channel: u8,
    /// 8-byte ASCII Track Name (padded with spaces).
    pub track_name: String,
    /// Destination Accompaniment Part (0x08..=0x0F corresponding to Rhy1..=Phr2).
    pub dest_channel: u8,
    /// Editable flag (`0x01` = editable, `0x00` = read-only).
    pub editable: bool,
    /// 12-bit active note bitmask across bytes 11..=12 (`0x0FFF` = all 12 pitch classes active).
    pub note_mute_mask: u16,
    /// 34-bit active chord bitmask across bytes 13..=17 (`0x03FFFFFFFF` or `0x07FFFFFFFF`).
    pub chord_mute_bytes: [u8; 5],
    /// Source Chord Root (`0x00` = C).
    pub source_chord_root: u8,
    /// Source Chord Type (`0x02` = Maj7).
    pub source_chord_type: u8,
    /// Note Transposition Rule (offset 20).
    pub ntr: NtrRule,
    /// Note Transposition Table (offset 21).
    pub ntt: NttTable,
    /// High Key limit for root transposition wrap-around (offset 22, e.g., `0x06` = F# or `0x08` = G#).
    pub high_key: u8,
    /// Lowest playable MIDI note before octave wrap-around (offset 23, 0..=127).
    pub note_limit_low: u8,
    /// Highest playable MIDI note before octave wrap-around (offset 24, 0..=127).
    pub note_limit_high: u8,
    /// Retrigger Rule on chord change (offset 25).
    pub rtr: RetriggerRule,
    /// Whether to also emit a 2-byte `Cntt` chunk for this channel (carries Bass-on bit `0x80`).
    pub bass_on: bool,
}

impl CtabEntry {
    /// Creates the canonical PSR-E583 SFF1 default `Ctab` configuration for a given accompaniment part.
    pub fn default_for_part(part: AccompanimentPart) -> Self {
        let ch = part.midi_channel();
        let is_rhy = part.is_rhythm();
        let (ntr, ntt, high_key, lo, hi, rtr, bass_on) = match part {
            AccompanimentPart::Rhy1 | AccompanimentPart::Rhy2 => (
                NtrRule::RootFixed,
                NttTable::Bypass,
                0,
                0,
                127,
                RetriggerRule::Stop,
                false,
            ),
            AccompanimentPart::Bass => (
                NtrRule::RootTransposition,
                NttTable::Bass,
                8,  // G# wrap point
                28, // E1
                64, // E4
                RetriggerRule::PitchShiftToRoot,
                true,
            ),
            AccompanimentPart::Chd1 | AccompanimentPart::Chd2 => (
                NtrRule::RootTransposition,
                NttTable::Chord,
                6,  // F# wrap point
                48, // C3
                84, // C6
                RetriggerRule::PitchShift,
                false,
            ),
            AccompanimentPart::Pad => (
                NtrRule::RootTransposition,
                NttTable::Chord,
                6,
                48, // C3
                96, // C7
                RetriggerRule::PitchShift,
                false,
            ),
            AccompanimentPart::Phr1 | AccompanimentPart::Phr2 => (
                NtrRule::RootTransposition,
                NttTable::Melody,
                6,
                48, // C3
                96, // C7
                RetriggerRule::Retrigger,
                false,
            ),
        };

        Self {
            source_channel: ch,
            track_name: part.default_track_name().to_string(),
            dest_channel: ch,
            editable: true,
            note_mute_mask: 0x0FFF,
            chord_mute_bytes: [if is_rhy { 0x07 } else { 0x03 }, 0xFF, 0xFF, 0xFF, 0xFF],
            source_chord_root: 0, // C
            source_chord_type: 2, // Maj7
            ntr,
            ntt,
            high_key,
            note_limit_low: lo,
            note_limit_high: hi,
            rtr,
            bass_on,
        }
    }

    /// Encodes the 27-byte `Ctab` payload wrapped in an 8-byte `"Ctab" + length` header (35 bytes total).
    pub fn to_bytes(&self) -> Vec<u8> {
        let mut payload = [0u8; 27];
        payload[0] = self.source_channel & 0x0F;

        let mut name_bytes = [b' '; 8];
        for (i, b) in self.track_name.as_bytes().iter().take(8).enumerate() {
            name_bytes[i] = *b;
        }
        payload[1..9].copy_from_slice(&name_bytes);

        payload[9] = self.dest_channel & 0x0F;
        payload[10] = u8::from(self.editable);
        payload[11] = ((self.note_mute_mask >> 8) & 0x0F) as u8;
        payload[12] = (self.note_mute_mask & 0xFF) as u8;
        payload[13..18].copy_from_slice(&self.chord_mute_bytes);
        payload[18] = self.source_chord_root & 0x0F;
        payload[19] = self.source_chord_type & 0x7F;
        payload[20] = self.ntr as u8;
        payload[21] = (self.ntt as u8) & 0x7F;
        payload[22] = self.high_key & 0x7F;
        payload[23] = self.note_limit_low & 0x7F;
        payload[24] = self.note_limit_high & 0x7F;
        payload[25] = self.rtr as u8;
        payload[26] = 0x00;

        let mut out = Vec::with_capacity(35);
        out.extend_from_slice(b"Ctab");
        out.extend_from_slice(&(payload.len() as u32).to_be_bytes());
        out.extend_from_slice(&payload);
        out
    }

    /// Encodes the 2-byte `Cntt` chunk (`"Cntt" + 0x00000002 + [dest_ch, ntt | bass_flag]`).
    pub fn to_cntt_bytes(&self) -> Vec<u8> {
        let mut out = Vec::with_capacity(10);
        out.extend_from_slice(b"Cntt");
        out.extend_from_slice(&2u32.to_be_bytes());
        out.push(self.dest_channel & 0x0F);
        let ntt_byte = ((self.ntt as u8) & 0x7F) | if self.bass_on { 0x80 } else { 0x00 };
        out.push(ntt_byte);
        out
    }

    /// Parses a `Ctab` payload (at least 26 bytes).
    pub fn from_payload(payload: &[u8]) -> Option<Self> {
        if payload.len() < 26 {
            return None;
        }
        let source_channel = payload[0] & 0x0F;
        let track_name = String::from_utf8_lossy(&payload[1..9]).to_string();
        let dest_channel = payload[9] & 0x0F;
        let editable = payload[10] != 0;
        let note_mute_mask = (((payload[11] & 0x0F) as u16) << 8) | (payload[12] as u16);
        let mut chord_mute_bytes = [0u8; 5];
        chord_mute_bytes.copy_from_slice(&payload[13..18]);
        let source_chord_root = payload[18];
        let source_chord_type = payload[19];
        let ntr = NtrRule::from_byte(payload[20]);
        let ntt = NttTable::from_byte(payload[21]);
        let high_key = payload[22];
        let note_limit_low = payload[23];
        let note_limit_high = payload[24];
        let rtr = RetriggerRule::from_byte(payload[25]);
        let bass_on = dest_channel == 10 || ntt == NttTable::Bass;

        Some(Self {
            source_channel,
            track_name,
            dest_channel,
            editable,
            note_mute_mask,
            chord_mute_bytes,
            source_chord_root,
            source_chord_type,
            ntr,
            ntt,
            high_key,
            note_limit_low,
            note_limit_high,
            rtr,
            bass_on,
        })
    }
}

/// A `CSEG` (Chord Segment) grouping one or more style sections with a set of `Ctab` channel rules.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CsegChunk {
    pub sections: Vec<StyleSection>,
    pub ctabs: Vec<CtabEntry>,
    pub include_cntt: bool,
}

impl CsegChunk {
    pub fn default_psr_e583() -> Self {
        Self {
            sections: StyleSection::ALL.to_vec(),
            ctabs: AccompanimentPart::ALL
                .iter()
                .map(|&p| CtabEntry::default_for_part(p))
                .collect(),
            include_cntt: true,
        }
    }

    pub fn to_bytes(&self) -> Vec<u8> {
        let mut body = Vec::new();

        // 1. Sdec chunk
        let sdec_str = if self.sections.is_empty() {
            "Main A,Main B".to_string()
        } else {
            self.sections
                .iter()
                .map(|s| s.as_marker_str())
                .collect::<Vec<_>>()
                .join(",")
        };
        let sdec_bytes = sdec_str.as_bytes();
        body.extend_from_slice(b"Sdec");
        body.extend_from_slice(&(sdec_bytes.len() as u32).to_be_bytes());
        body.extend_from_slice(sdec_bytes);

        // 2. Ctab chunks
        for ctab in &self.ctabs {
            body.extend_from_slice(&ctab.to_bytes());
        }

        // 3. Optional Cntt chunks
        if self.include_cntt {
            for ctab in &self.ctabs {
                body.extend_from_slice(&ctab.to_cntt_bytes());
            }
        }

        let mut out = Vec::with_capacity(8 + body.len());
        out.extend_from_slice(b"CSEG");
        out.extend_from_slice(&(body.len() as u32).to_be_bytes());
        out.extend_from_slice(&body);
        out
    }
}

/// Root `CASM` container chunk.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CasmChunk {
    pub segments: Vec<CsegChunk>,
}

impl Default for CasmChunk {
    fn default() -> Self {
        Self {
            segments: vec![CsegChunk::default_psr_e583()],
        }
    }
}

impl CasmChunk {
    pub fn to_bytes(&self) -> Vec<u8> {
        let mut body = Vec::new();
        for seg in &self.segments {
            body.extend_from_slice(&seg.to_bytes());
        }
        let mut out = Vec::with_capacity(8 + body.len());
        out.extend_from_slice(b"CASM");
        out.extend_from_slice(&(body.len() as u32).to_be_bytes());
        out.extend_from_slice(&body);
        out
    }

    /// Parses a `CASM` chunk from a raw byte slice starting at `"CASM"`.
    pub fn from_bytes(data: &[u8]) -> Result<Self, String> {
        if data.len() < 8 || &data[0..4] != b"CASM" {
            return Err("Missing 'CASM' four-byte header".to_string());
        }
        let casm_len = u32::from_be_bytes([data[4], data[5], data[6], data[7]]) as usize;
        if casm_len == 0 {
            return Ok(Self { segments: Vec::new() });
        }
        let end = (8 + casm_len).min(data.len());
        let mut pos = 8;
        let mut segments = Vec::new();

        while pos + 8 <= end {
            let tag = &data[pos..pos + 4];
            let sz = u32::from_be_bytes([data[pos + 4], data[pos + 5], data[pos + 6], data[pos + 7]])
                as usize;
            pos += 8;
            if pos + sz > end {
                break;
            }
            if tag == b"CSEG" {
                let seg_end = pos + sz;
                let mut q = pos;
                let mut sections = Vec::new();
                let mut ctabs = Vec::new();
                let mut include_cntt = false;

                while q + 8 <= seg_end {
                    let sub_tag = &data[q..q + 4];
                    let sub_sz =
                        u32::from_be_bytes([data[q + 4], data[q + 5], data[q + 6], data[q + 7]])
                            as usize;
                    q += 8;
                    if q + sub_sz > seg_end {
                        break;
                    }
                    let payload = &data[q..q + sub_sz];
                    if sub_tag == b"Sdec" {
                        let s = String::from_utf8_lossy(payload);
                        for part in s.split([',', ';', '\0']) {
                            if let Some(sec) = StyleSection::from_marker_str(part)
                                && !sections.contains(&sec)
                            {
                                sections.push(sec);
                            }
                        }
                    } else if sub_tag == b"Ctab" {
                        if let Some(ct) = CtabEntry::from_payload(payload) {
                            ctabs.push(ct);
                        }
                    } else if sub_tag == b"Cntt" && payload.len() >= 2 {
                        include_cntt = true;
                        let ch = payload[0] & 0x0F;
                        let bass_flag = (payload[1] & 0x80) != 0;
                        if let Some(ct) = ctabs.iter_mut().find(|c| c.dest_channel == ch) {
                            ct.bass_on = bass_flag;
                        }
                    }
                    q += sub_sz;
                }

                segments.push(CsegChunk {
                    sections,
                    ctabs,
                    include_cntt,
                });
            }
            pos += sz;
        }

        Ok(Self { segments })
    }

    /// Transposes a C-Maj7 source `note` on `channel` (0..=15) for `section` according to the active `Ctab`
    /// rules (`NTR`, `NTT`, `High Key`, `Note Limit Low/High`) and the live `DetectedChord`.
    pub fn transpose_note(
        &self,
        section: StyleSection,
        channel: u8,
        note: u8,
        chord: DetectedChord,
    ) -> u8 {
        let ch = channel & 0x0F;
        let ctab = self
            .segments
            .iter()
            .find(|seg| seg.sections.contains(&section) || seg.sections.is_empty())
            .or_else(|| self.segments.first())
            .and_then(|seg| seg.ctabs.iter().find(|c| c.dest_channel == ch));

        if let Some(ct) = ctab {
            ct.transpose_note(note, chord)
        } else if ch == 8 || ch == 9 {
            note
        } else {
            CtabEntry::default_for_part(
                AccompanimentPart::from_midi_channel(ch).unwrap_or(AccompanimentPart::Chd1),
            )
            .transpose_note(note, chord)
        }
    }
}

impl CtabEntry {
    /// Applies this channel's `NTR`, `NTT`, `High Key`, and `Note Limit Low/High` rules to a C-Maj7 source note.
    pub fn transpose_note(&self, note: u8, chord: DetectedChord) -> u8 {
        if self.ntr == NtrRule::RootFixed && self.ntt == NttTable::Bypass {
            return note;
        }

        let src_pc = note % 12;
        let base_octave_c = (note as i32) - (src_pc as i32);

        // 1. Interval adjustment via NTT table (source pattern composed in C Maj7: C=0, E=4, G=7, B=11)
        let interval: i32 = match src_pc {
            4 => match chord.quality {
                ChordQuality::Minor | ChordQuality::Min7 | ChordQuality::Dim => 3, // E -> Eb
                ChordQuality::Sus4 => 5,                                           // E -> F
                _ => 4,
            },
            7 => match chord.quality {
                ChordQuality::Dim => 6, // G -> Gb
                _ => 7,
            },
            11 => match chord.quality {
                ChordQuality::Maj7 => 11,                           // Keep B
                ChordQuality::Dom7 | ChordQuality::Min7 => 10,      // B -> Bb
                ChordQuality::Dim => 9,                             // B -> A (dim7)
                ChordQuality::Major | ChordQuality::Minor | ChordQuality::Sus4 => {
                    if self.ntt == NttTable::Bass {
                        7 // In Bass table, Maj7th degree resolves to 5th (G) on triads
                    } else {
                        12 // In Chord/Melody table, C4-E4-G4-B4 becomes C4-E4-G4-C5 on triads
                    }
                }
            },
            other => other as i32,
        };

        // 2. Root shift via NTR rule & High Key wrap point
        let root_shift = if self.ntr == NtrRule::RootFixed {
            0
        } else {
            let mut r = (chord.root % 12) as i32;
            let hk = self.high_key.min(11) as i32;
            if r > hk {
                r -= 12;
            }
            r
        };

        let mut transposed = base_octave_c + interval + root_shift;

        // 3. Octave wrap into [note_limit_low ..= note_limit_high]
        let lo = self.note_limit_low.min(self.note_limit_high) as i32;
        let hi = self.note_limit_high.max(self.note_limit_low) as i32;
        if hi >= lo {
            while transposed < lo && transposed + 12 <= 127 {
                transposed += 12;
            }
            while transposed > hi && transposed - 12 >= 0 {
                transposed -= 12;
            }
        }

        transposed.clamp(0, 127) as u8
    }
}

pub const PITCH_CLASS_NAMES: [&str; 12] = [
    "C", "C#", "D", "Eb", "E", "F", "F#", "G", "Ab", "A", "Bb", "B",
];

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum ChordQuality {
    Major,
    Minor,
    Dom7,
    Maj7,
    Min7,
    Dim,
    Sus4,
}

impl ChordQuality {
    pub const ALL: &'static [ChordQuality] = &[
        ChordQuality::Major,
        ChordQuality::Minor,
        ChordQuality::Dom7,
        ChordQuality::Maj7,
        ChordQuality::Min7,
        ChordQuality::Dim,
        ChordQuality::Sus4,
    ];

    pub const fn suffix(&self) -> &'static str {
        match self {
            ChordQuality::Major => "",
            ChordQuality::Minor => "m",
            ChordQuality::Dom7 => "7",
            ChordQuality::Maj7 => "Maj7",
            ChordQuality::Min7 => "m7",
            ChordQuality::Dim => "dim",
            ChordQuality::Sus4 => "sus4",
        }
    }
}

/// Represents the active left-hand chord in Live USB-C Arranger Mode.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct DetectedChord {
    /// Pitch class root (0 = C, 1 = C#, ..., 11 = B).
    pub root: u8,
    pub quality: ChordQuality,
}

impl Default for DetectedChord {
    fn default() -> Self {
        Self {
            root: 0, // C
            quality: ChordQuality::Major,
        }
    }
}

impl DetectedChord {
    pub fn display_name(&self) -> String {
        format!(
            "{}{}",
            PITCH_CLASS_NAMES[(self.root % 12) as usize],
            self.quality.suffix()
        )
    }
}

/// Recognizes a Yamaha ACMP chord from held left-hand MIDI notes (supports both Single-Finger
/// and full Multi-Fingered chord voicings).
pub fn recognize_chord(held_notes: &[u8]) -> Option<DetectedChord> {
    if held_notes.is_empty() {
        return None;
    }
    let mut sorted = held_notes.to_vec();
    sorted.sort_unstable();
    sorted.dedup();

    // 1-Note Single-Finger -> Major triad on that root
    if sorted.len() == 1 {
        return Some(DetectedChord {
            root: sorted[0] % 12,
            quality: ChordQuality::Major,
        });
    }

    // 2-Note Smart / Two-Finger recognition
    if sorted.len() == 2 {
        let n0 = sorted[0];
        let n1 = sorted[1];
        let iv = (n1.saturating_sub(n0)) % 12;
        return Some(match iv {
            3 => DetectedChord {
                root: n0 % 12,
                quality: ChordQuality::Minor,
            },
            4 | 7 => DetectedChord {
                root: n0 % 12,
                quality: ChordQuality::Major,
            },
            10 => DetectedChord {
                root: n0 % 12,
                quality: ChordQuality::Dom7,
            },
            11 => DetectedChord {
                root: n0 % 12,
                quality: ChordQuality::Maj7,
            },
            2 => DetectedChord {
                // Yamaha 2-finger 7th (e.g. Bb + C = C7)
                root: n1 % 12,
                quality: ChordQuality::Dom7,
            },
            1 => DetectedChord {
                // Yamaha 2-finger Maj7 (e.g. B + C = CMaj7)
                root: n1 % 12,
                quality: ChordQuality::Maj7,
            },
            8 => DetectedChord {
                root: n1 % 12,
                quality: ChordQuality::Major,
            },
            9 => DetectedChord {
                root: n1 % 12,
                quality: ChordQuality::Minor,
            },
            5 => DetectedChord {
                root: n1 % 12,
                quality: ChordQuality::Major,
            },
            _ => DetectedChord {
                root: n0 % 12,
                quality: ChordQuality::Major,
            },
        });
    }

    // 3+ Notes Full Fingered recognition
    let mut mask = 0u16;
    for &n in &sorted {
        mask |= 1 << (n % 12);
    }

    let bass_pc = sorted[0] % 12;
    let templates: &[(u16, ChordQuality)] = &[
        ((1 << 0) | (1 << 4) | (1 << 7) | (1 << 11), ChordQuality::Maj7),
        ((1 << 0) | (1 << 4) | (1 << 11), ChordQuality::Maj7),
        ((1 << 0) | (1 << 4) | (1 << 7) | (1 << 10), ChordQuality::Dom7),
        ((1 << 0) | (1 << 4) | (1 << 10), ChordQuality::Dom7),
        ((1 << 0) | (1 << 3) | (1 << 7) | (1 << 10), ChordQuality::Min7),
        ((1 << 0) | (1 << 3) | (1 << 10), ChordQuality::Min7),
        ((1 << 0) | (1 << 4) | (1 << 7), ChordQuality::Major),
        ((1 << 0) | (1 << 3) | (1 << 7), ChordQuality::Minor),
        ((1 << 0) | (1 << 3) | (1 << 6), ChordQuality::Dim),
        ((1 << 0) | (1 << 5) | (1 << 7), ChordQuality::Sus4),
    ];

    // Check candidate roots starting with the lowest held bass note
    let mut candidate_roots = vec![bass_pc];
    for r in 0..12u8 {
        if r != bass_pc {
            candidate_roots.push(r);
        }
    }

    for &(tpl, quality) in templates {
        for &r in &candidate_roots {
            let rotated = ((tpl << r) | (tpl >> (12 - r))) & 0x0FFF;
            if (mask & rotated) == rotated {
                return Some(DetectedChord { root: r, quality });
            }
        }
    }

    Some(DetectedChord {
        root: bass_pc,
        quality: ChordQuality::Major,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_casm_roundtrip_and_golden_bytes() {
        let casm = CasmChunk::default();
        let bytes = casm.to_bytes();
        assert_eq!(&bytes[0..4], b"CASM");

        let parsed = CasmChunk::from_bytes(&bytes).expect("CASM should parse cleanly");
        assert_eq!(parsed.segments.len(), 1);
        assert_eq!(parsed.segments[0].sections.len(), StyleSection::ALL.len());
        assert_eq!(parsed.segments[0].ctabs.len(), 8);

        // Verify Bass Ctab golden offsets
        let bass_ctab = &parsed.segments[0].ctabs[2];
        assert_eq!(bass_ctab.source_channel, 10);
        assert_eq!(bass_ctab.dest_channel, 10);
        assert_eq!(bass_ctab.ntr, NtrRule::RootTransposition);
        assert_eq!(bass_ctab.ntt, NttTable::Bass);
        assert_eq!(bass_ctab.rtr, RetriggerRule::PitchShiftToRoot);
        assert!(bass_ctab.bass_on);
    }

    #[test]
    fn test_chord_recognition_and_casm_transposition() {
        // A Minor triad (A2=45, C3=48, E3=52)
        let am = recognize_chord(&[45, 48, 52]).expect("Should detect Am");
        assert_eq!(am.root, 9); // A
        assert_eq!(am.quality, ChordQuality::Minor);
        assert_eq!(am.display_name(), "Am");

        // F Major 7 (F2=41, A2=45, C3=48, E3=52)
        let fmaj7 = recognize_chord(&[41, 45, 48, 52]).expect("Should detect FMaj7");
        assert_eq!(fmaj7.root, 5); // F
        assert_eq!(fmaj7.quality, ChordQuality::Maj7);

        let casm = CasmChunk::default();
        // Drums (Ch 10 / index 9) must never transpose
        assert_eq!(casm.transpose_note(StyleSection::MainA, 9, 36, am), 36);

        // Chd1 (Ch 12 / index 11): E4 (64) in C Maj7 -> C4 (60) in A Minor (root A=-3 semitones + minor 3rd +3 = C4!)
        let chd_third = casm.transpose_note(StyleSection::MainA, 11, 64, am);
        assert_eq!(chd_third % 12, 0); // Pitch class C (minor 3rd of A minor)
    }
}
