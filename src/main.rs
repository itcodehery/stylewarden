mod casm;
mod midi;
mod project;
mod voices;

use casm::{
    AccompanimentPart, ChordQuality, DetectedChord, NtrRule, NttTable, PITCH_CLASS_NAMES,
    RetriggerRule, StyleSection, recognize_chord,
};
use eframe::egui;
use midir::{MidiInput, MidiInputConnection, MidiOutput, MidiOutputConnection};
use project::{
    DrumRow, SectionPattern, SongSlicerState, StylePreset, StyleProject, StyValidationReport,
    validate_sty_bytes,
};
use std::sync::{Arc, Mutex};
use std::time::Instant;
use voices::{DRUM_KIT_NOTES, PSR_E583_VOICES};

fn main() -> eframe::Result<()> {
    let args: Vec<String> = std::env::args().collect();
    if let Some(pos) = args.iter().position(|a| a == "--export-presets") {
        let out_dir = args
            .get(pos + 1)
            .cloned()
            .unwrap_or_else(|| "exports".to_string());
        let _ = std::fs::create_dir_all(&out_dir);
        for &preset in StylePreset::ALL {
            let mut proj = StyleProject::default();
            proj.apply_preset(preset);
            let bytes = proj.compile_sty_bytes();
            let report = validate_sty_bytes(&bytes);
            let path = std::path::Path::new(&out_dir).join(format!("{}.sty", proj.style_name));
            std::fs::write(&path, &bytes).expect("Failed to write preset .sty file");
            println!(
                "Exported {:<24} ({:>5} bytes, {}/{} checks passed) -> {}",
                format!("{}.sty", proj.style_name),
                bytes.len(),
                report.checks.iter().filter(|c| c.passed).count(),
                report.checks.len(),
                path.display()
            );
        }
        return Ok(());
    }

    let native_options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_title("StyleWarden — Yamaha PSR-E583 SFF1 (.sty) Style Creator & Drum Machine")
            .with_inner_size([1360.0, 880.0])
            .with_min_inner_size([1024.0, 680.0]),
        ..Default::default()
    };

    eframe::run_native(
        "StyleWarden",
        native_options,
        Box::new(|cc| Ok(Box::new(StyleWardenApp::new(cc)))),
    )
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum WorkspaceTab {
    DrumMachine,
    AccompanimentAndSlicer,
    VoiceMixer,
    CasmAndInspector,
}

/// Shared state updated by the USB-MIDI Input callback when the user plays keys on the PSR-E583.
struct LiveMidiInState {
    held_left_notes: Vec<u8>,
    split_point: u8, // Default 54 (F#3 on MIDI / F#2 on Yamaha Accompaniment split)
    detected_chord: Option<DetectedChord>,
    note_on_trigger: bool,
}

impl Default for LiveMidiInState {
    fn default() -> Self {
        Self {
            held_left_notes: Vec::new(),
            split_point: 54, // F#3
            detected_chord: None,
            note_on_trigger: false,
        }
    }
}

struct StyleWardenApp {
    project: StyleProject,
    active_section: StyleSection,
    active_tab: WorkspaceTab,

    // Drum Machine UI state
    brush_velocity: u8,
    selected_new_drum_idx: usize,
    selected_new_drum_part: AccompanimentPart,
    copied_section: Option<SectionPattern>,

    // Accompaniment & Song Slicer UI state
    selected_accomp_part: AccompanimentPart,
    slicer: SongSlicerState,

    // Voice Browser UI state
    voice_search_query: String,
    voice_category_filter: String,
    editing_voice_part: AccompanimentPart,

    // Binary Inspector cache
    cached_sty_bytes: Vec<u8>,
    cached_report: StyValidationReport,
    inspector_dirty: bool,

    // Real-time Step Transport & bidirectional USB-MIDI (midir In + Out)
    is_playing: bool,
    current_step: usize,
    last_step_instant: Instant,
    midi_port_names: Vec<String>,
    selected_midi_port: Option<usize>,
    midi_conn: Option<MidiOutputConnection>,
    /// Active melodic/accompaniment notes currently sounding over USB-MIDI:
    /// `(channel, orig_cmaj7_note, sounding_transposed_note, remaining_steps)`.
    active_midi_notes: Vec<(u8, u8, u8, usize)>,

    // Live USB-C Arranger Mode (MIDI In Chord Recognition & Auto-Fill Transitions)
    egui_ctx: egui::Context,
    midi_in_port_names: Vec<String>,
    selected_midi_in_port: Option<usize>,
    midi_in_conn: Option<MidiInputConnection<()>>,
    live_midi_in: Arc<Mutex<LiveMidiInState>>,
    acmp_on: bool,
    sync_start: bool,
    auto_fill: bool,
    queued_next_section: Option<StyleSection>,
    current_chord: DetectedChord,

    // Layout & Sidebar state
    show_left_sidebar: bool,

    status_banner: String,
}

fn studio_card_frame() -> egui::Frame {
    egui::Frame::new()
        .fill(egui::Color32::from_rgb(26, 30, 39))
        .stroke(egui::Stroke::new(1.0, egui::Color32::from_rgb(44, 51, 66)))
        .corner_radius(egui::CornerRadius::same(8))
        .inner_margin(egui::Margin::symmetric(16, 14))
}

impl StyleWardenApp {
    fn new(cc: &eframe::CreationContext<'_>) -> Self {
        // Configure dark studio visual theme with generous spacing and margins
        let mut style = (*cc.egui_ctx.style()).clone();
        style.spacing.item_spacing = egui::vec2(10.0, 8.0);
        style.spacing.button_padding = egui::vec2(10.0, 6.0);
        style.spacing.window_margin = egui::Margin::same(14);
        style.spacing.menu_margin = egui::Margin::same(10);
        style.spacing.indent = 16.0;

        let mut visuals = egui::Visuals::dark();
        visuals.window_fill = egui::Color32::from_rgb(18, 20, 26);
        visuals.panel_fill = egui::Color32::from_rgb(20, 23, 30);
        visuals.faint_bg_color = egui::Color32::from_rgb(28, 32, 42);
        visuals.extreme_bg_color = egui::Color32::from_rgb(13, 15, 20);
        visuals.widgets.noninteractive.corner_radius = egui::CornerRadius::same(6);
        visuals.widgets.inactive.corner_radius = egui::CornerRadius::same(6);
        visuals.widgets.hovered.corner_radius = egui::CornerRadius::same(6);
        visuals.widgets.active.corner_radius = egui::CornerRadius::same(6);
        visuals.widgets.open.corner_radius = egui::CornerRadius::same(6);
        style.visuals = visuals;
        cc.egui_ctx.set_style(style);

        let project = StyleProject::default();
        let cached_sty_bytes = project.compile_sty_bytes();
        let cached_report = validate_sty_bytes(&cached_sty_bytes);

        let mut app = Self {
            project,
            active_section: StyleSection::MainA,
            active_tab: WorkspaceTab::DrumMachine,
            brush_velocity: 105,
            selected_new_drum_idx: 0,
            selected_new_drum_part: AccompanimentPart::Rhy2,
            copied_section: None,
            selected_accomp_part: AccompanimentPart::Bass,
            slicer: SongSlicerState::default(),
            voice_search_query: String::new(),
            voice_category_filter: "All".to_string(),
            editing_voice_part: AccompanimentPart::Rhy2,
            cached_sty_bytes,
            cached_report,
            inspector_dirty: false,
            is_playing: false,
            current_step: 0,
            last_step_instant: Instant::now(),
            midi_port_names: Vec::new(),
            selected_midi_port: None,
            midi_conn: None,
            active_midi_notes: Vec::new(),
            egui_ctx: cc.egui_ctx.clone(),
            midi_in_port_names: Vec::new(),
            selected_midi_in_port: None,
            midi_in_conn: None,
            live_midi_in: Arc::new(Mutex::new(LiveMidiInState::default())),
            acmp_on: true,
            sync_start: false,
            auto_fill: true,
            queued_next_section: None,
            current_chord: DetectedChord::default(),
            show_left_sidebar: true,
            status_banner:
                "Ready — PSR-E583 SFF1 Engine initialized (890 Voices + 40 Drum Kits loaded)."
                    .to_string(),
        };
        app.refresh_midi_ports();
        app
    }

    fn mark_dirty(&mut self) {
        self.inspector_dirty = true;
    }

    fn ensure_compiled(&mut self) {
        if self.inspector_dirty {
            self.cached_sty_bytes = self.project.compile_sty_bytes();
            self.cached_report = validate_sty_bytes(&self.cached_sty_bytes);
            self.inspector_dirty = false;
        }
    }

    fn refresh_midi_ports(&mut self) {
        self.midi_port_names.clear();
        if let Ok(midi_out) = MidiOutput::new("StyleWarden-Probe-Out") {
            for p in midi_out.ports() {
                if let Ok(name) = midi_out.port_name(&p) {
                    self.midi_port_names.push(name);
                }
            }
        }
        self.midi_in_port_names.clear();
        if let Ok(midi_in) = MidiInput::new("StyleWarden-Probe-In") {
            for p in midi_in.ports() {
                if let Ok(name) = midi_in.port_name(&p) {
                    self.midi_in_port_names.push(name);
                }
            }
        }

        // Automatically connect USB-MIDI Out to PSR-E583 ("Digital Keyboard" / "PSR" / "Yamaha")
        if self.midi_conn.is_none() {
            if let Some(idx) = self.midi_port_names.iter().position(|n| {
                let lower = n.to_ascii_lowercase();
                lower.contains("digital keyboard")
                    || lower.contains("psr")
                    || lower.contains("yamaha")
            }) {
                self.connect_midi_port(idx);
            }
        }

        // Automatically connect USB-MIDI In from PSR-E583 for Live Left-Hand Chord Recognition
        if self.midi_in_conn.is_none() {
            if let Some(idx) = self.midi_in_port_names.iter().position(|n| {
                let lower = n.to_ascii_lowercase();
                lower.contains("digital keyboard")
                    || lower.contains("psr")
                    || lower.contains("yamaha")
            }) {
                self.connect_midi_in_port(idx);
            }
        }
    }

    fn connect_midi_in_port(&mut self, port_idx: usize) {
        self.midi_in_conn = None;
        self.selected_midi_in_port = None;
        if let Ok(midi_in) = MidiInput::new("StyleWarden-Arranger-In") {
            let ports = midi_in.ports();
            if let Some(port) = ports.get(port_idx) {
                let port_name = midi_in
                    .port_name(port)
                    .unwrap_or_else(|_| "MIDI In".to_string());
                let state_arc = Arc::clone(&self.live_midi_in);
                let ctx_clone = self.egui_ctx.clone();

                if let Ok(conn) = midi_in.connect(
                    port,
                    "stylewarden-psr-e583-in",
                    move |_timestamp, msg, _| {
                        if msg.len() >= 3 {
                            let cmd = msg[0] & 0xF0;
                            let note = msg[1] & 0x7F;
                            let vel = msg[2] & 0x7F;
                            if let Ok(mut st) = state_arc.lock() {
                                if cmd == 0x90 && vel > 0 {
                                    if note <= st.split_point {
                                        if !st.held_left_notes.contains(&note) {
                                            st.held_left_notes.push(note);
                                        }
                                        st.detected_chord = recognize_chord(&st.held_left_notes);
                                        st.note_on_trigger = true;
                                        ctx_clone.request_repaint();
                                    }
                                } else if cmd == 0x80 || (cmd == 0x90 && vel == 0) {
                                    if st.held_left_notes.contains(&note) {
                                        st.held_left_notes.retain(|&n| n != note);
                                        if !st.held_left_notes.is_empty() {
                                            st.detected_chord =
                                                recognize_chord(&st.held_left_notes);
                                        }
                                        ctx_clone.request_repaint();
                                    }
                                }
                            }
                        }
                    },
                    (),
                ) {
                    self.midi_in_conn = Some(conn);
                    self.selected_midi_in_port = Some(port_idx);
                    self.status_banner = format!(
                        "Live Arranger Ready — USB-MIDI In & Out connected to '{}'",
                        port_name
                    );
                }
            }
        }
    }

    fn sync_midi_channel_configs(&mut self) {
        let Some(conn) = &mut self.midi_conn else {
            return;
        };
        for cfg in &self.project.channel_configs {
            let ch = cfg.part.midi_channel() & 0x0F;
            let _ = conn.send(&[0xB0 | ch, 0, cfg.bank_msb & 0x7F]);
            let _ = conn.send(&[0xB0 | ch, 32, cfg.bank_lsb & 0x7F]);
            let _ = conn.send(&[0xC0 | ch, cfg.program_change & 0x7F]);
            let _ = conn.send(&[0xB0 | ch, 7, if cfg.muted { 0 } else { cfg.volume & 0x7F }]);
            let _ = conn.send(&[0xB0 | ch, 10, cfg.pan & 0x7F]);
            let _ = conn.send(&[0xB0 | ch, 91, cfg.reverb & 0x7F]);
            let _ = conn.send(&[0xB0 | ch, 93, cfg.chorus & 0x7F]);
        }
    }

    fn stop_all_midi_notes(&mut self) {
        let Some(conn) = &mut self.midi_conn else {
            self.active_midi_notes.clear();
            return;
        };
        for &(ch, _, sounding_note, _) in &self.active_midi_notes {
            let _ = conn.send(&[0x80 | (ch & 0x0F), sounding_note & 0x7F, 0]);
        }
        self.active_midi_notes.clear();
        for ch in 8..16u8 {
            // CC#123 All Notes Off
            let _ = conn.send(&[0xB0 | ch, 123, 0]);
        }
    }

    /// Re-voices any currently sustained accompaniment notes immediately when the user plays a new left-hand chord!
    fn revoice_held_accompaniment_notes(&mut self) {
        if !self.is_playing || !self.acmp_on {
            return;
        }
        let Some(conn) = &mut self.midi_conn else {
            return;
        };
        for entry in &mut self.active_midi_notes {
            let (ch, orig_note, old_sounding, _rem) = *entry;
            let new_sounding = self.project.casm.transpose_note(
                self.active_section,
                ch,
                orig_note,
                self.current_chord,
            ) & 0x7F;
            if new_sounding != old_sounding {
                let _ = conn.send(&[0x80 | (ch & 0x0F), old_sounding & 0x7F, 0]);
                let _ = conn.send(&[0x90 | (ch & 0x0F), new_sounding, 80]);
                entry.2 = new_sounding;
            }
        }
    }

    fn connect_midi_port(&mut self, port_idx: usize) {
        self.stop_all_midi_notes();
        self.midi_conn = None;
        self.selected_midi_port = None;
        if let Ok(midi_out) = MidiOutput::new("StyleWarden-Out") {
            let ports = midi_out.ports();
            if let Some(port) = ports.get(port_idx) {
                let port_name = midi_out
                    .port_name(port)
                    .unwrap_or_else(|_| "MIDI Port".to_string());
                if let Ok(conn) = midi_out.connect(port, "stylewarden-psr-e583") {
                    self.midi_conn = Some(conn);
                    self.selected_midi_port = Some(port_idx);
                    self.sync_midi_channel_configs();
                    self.status_banner = format!("Connected USB-MIDI Out to '{}'", port_name);
                }
            }
        }
    }

    fn trigger_step_midi(&mut self, step: usize) {
        let Some(conn) = &mut self.midi_conn else {
            return;
        };
        let sec_pat = self
            .project
            .sections
            .iter()
            .find(|s| s.section == self.active_section);
        let Some(sec_pat) = sec_pat else {
            return;
        };

        // 1. Release any sustained accompaniment notes whose step duration has completed (or on loop wrap at step 0)
        if step == 0 {
            for &(ch, _, sounding_note, _) in &self.active_midi_notes {
                let _ = conn.send(&[0x80 | (ch & 0x0F), sounding_note & 0x7F, 0]);
            }
            self.active_midi_notes.clear();
        } else {
            let mut still_active = Vec::with_capacity(self.active_midi_notes.len());
            for (ch, orig_note, sounding_note, rem) in self.active_midi_notes.drain(..) {
                if rem <= 1 {
                    let _ = conn.send(&[0x80 | (ch & 0x0F), sounding_note & 0x7F, 0]);
                } else {
                    still_active.push((ch, orig_note, sounding_note, rem - 1));
                }
            }
            self.active_midi_notes = still_active;
        }

        // 2. Trigger Drum Machine hits (Rhy1 Ch 9 & Rhy2 Ch 10)
        for row in &sec_pat.drum_rows {
            if row.muted {
                continue;
            }
            let ch_muted = self
                .project
                .channel_configs
                .iter()
                .find(|c| c.part == row.part)
                .is_some_and(|c| c.muted);
            if ch_muted {
                continue;
            }
            if let Some(&vel) = row.steps.get(step)
                && vel > 0
            {
                let ch = row.part.midi_channel() & 0x0F;
                let _ = conn.send(&[0x80 | ch, row.midi_note & 0x7F, 0]);
                let _ = conn.send(&[0x90 | ch, row.midi_note & 0x7F, vel.clamp(1, 127)]);
            }
        }

        // 3. Trigger Melodic/Harmonic Accompaniment Notes (Bass, Chd1, Chd2, Pad, Phr1, Phr2 on Ch 11-16)
        // transposed in real time using the CASM NTR/NTT rules and active left-hand chord!
        let tpq = self.project.ticks_per_quarter.max(96) as u32;
        let step_ticks = tpq / 4;
        let win_start = (step as u32) * step_ticks;
        let win_end = win_start + step_ticks;

        for n in &sec_pat.accompaniment_notes {
            if n.start_tick >= win_start && n.start_tick < win_end {
                if let Some(part) = AccompanimentPart::from_midi_channel(n.channel)
                    && self
                        .project
                        .channel_configs
                        .iter()
                        .find(|c| c.part == part)
                        .is_some_and(|c| c.muted)
                {
                    continue;
                }
                let ch = n.channel & 0x0F;
                let orig_note = n.note & 0x7F;
                let sounding_note = if self.acmp_on {
                    self.project.casm.transpose_note(
                        self.active_section,
                        ch,
                        orig_note,
                        self.current_chord,
                    ) & 0x7F
                } else {
                    orig_note
                };

                // If this exact (ch, sounding_note) is already active, send NoteOff before re-triggering
                if let Some(pos) = self
                    .active_midi_notes
                    .iter()
                    .position(|&(c, _, s, _)| c == ch && s == sounding_note)
                {
                    let _ = conn.send(&[0x80 | ch, sounding_note, 0]);
                    self.active_midi_notes.swap_remove(pos);
                }
                let _ = conn.send(&[0x90 | ch, sounding_note, n.velocity.clamp(1, 127)]);
                let dur_steps = ((n.duration_ticks + step_ticks / 2) / step_ticks).max(1) as usize;
                self.active_midi_notes
                    .push((ch, orig_note, sounding_note, dur_steps));
            }
        }
    }

    fn export_sty_dialog(&mut self) {
        self.ensure_compiled();
        let default_name = format!("{}.sty", self.project.style_name.trim());
        if let Some(path) = rfd::FileDialog::new()
            .set_title("Export Yamaha PSR-E583 SFF1 Style (.sty)")
            .set_file_name(&default_name)
            .add_filter("Yamaha Style File (.sty)", &["sty", "STY"])
            .save_file()
        {
            match std::fs::write(&path, &self.cached_sty_bytes) {
                Ok(_) => {
                    self.status_banner = format!(
                        "Exported {} ({} bytes, 10/10 PSR-E583 checks passed) -> {}",
                        default_name,
                        self.cached_sty_bytes.len(),
                        path.display()
                    );
                }
                Err(e) => {
                    self.status_banner = format!("Failed to write .sty file: {}", e);
                }
            }
        }
    }

    fn import_sty_or_mid_dialog(&mut self) {
        if let Some(path) = rfd::FileDialog::new()
            .set_title("Open .sty Style or .mid MIDI File")
            .add_filter("Style & MIDI Files", &["sty", "STY", "mid", "MID", "midi"])
            .pick_file()
        {
            match std::fs::read(&path) {
                Ok(bytes) => {
                    let ext = path
                        .extension()
                        .and_then(|e| e.to_str())
                        .unwrap_or("")
                        .to_ascii_lowercase();
                    let stem = path
                        .file_stem()
                        .and_then(|s| s.to_str())
                        .unwrap_or("ImportedStyle");

                    if ext == "sty" {
                        match StyleProject::import_sty_bytes(&bytes, stem) {
                            Ok(proj) => {
                                self.project = proj;
                                self.mark_dirty();
                                self.status_banner =
                                    format!("Imported SFF1 Style '{}' from {}", stem, path.display());
                            }
                            Err(e) => {
                                self.status_banner = format!("Error importing .sty: {}", e);
                            }
                        }
                    } else {
                        // Load into Song-to-Style Slicer and switch to Slicer tab
                        match midi::parse_smf_bytes(&bytes) {
                            Ok(parsed) => {
                                let bars = parsed.total_bars();
                                self.slicer.file_name = path
                                    .file_name()
                                    .and_then(|n| n.to_str())
                                    .unwrap_or("song.mid")
                                    .to_string();
                                self.slicer.status_message = format!(
                                    "Loaded '{}' (Format {}, {} tracks, {:.1} BPM, {} bars, {} notes)",
                                    self.slicer.file_name,
                                    parsed.format,
                                    parsed.num_tracks,
                                    parsed.bpm,
                                    bars,
                                    parsed.notes.len()
                                );
                                self.slicer.parsed = Some(parsed);
                                self.active_tab = WorkspaceTab::AccompanimentAndSlicer;
                                self.status_banner = self.slicer.status_message.clone();
                            }
                            Err(e) => {
                                self.status_banner = format!("Error parsing MIDI file: {}", e);
                            }
                        }
                    }
                }
                Err(e) => {
                    self.status_banner = format!("Could not read file: {}", e);
                }
            }
        }
    }
}

impl eframe::App for StyleWardenApp {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        // 1. Poll live left-hand chord events from the PSR-E583 USB-MIDI In connection
        let mut chord_changed = false;
        let mut triggered_sync_start = false;
        if let Ok(mut st) = self.live_midi_in.lock() {
            if let Some(det) = st.detected_chord {
                if det != self.current_chord {
                    self.current_chord = det;
                    chord_changed = true;
                }
            }
            if st.note_on_trigger {
                st.note_on_trigger = false;
                if self.sync_start && !self.is_playing {
                    triggered_sync_start = true;
                }
            }
        }

        if triggered_sync_start {
            self.is_playing = true;
            self.current_step = 0;
            self.last_step_instant = Instant::now();
            self.sync_midi_channel_configs();
            self.trigger_step_midi(0);
        } else if chord_changed {
            self.revoice_held_accompaniment_notes();
        }

        // 2. Update real-time step sequencer clock if playing
        if self.is_playing {
            let step_duration_secs = (60.0 / self.project.bpm.clamp(30.0, 300.0)) / 4.0;
            let elapsed = self.last_step_instant.elapsed().as_secs_f32();
            if elapsed >= step_duration_secs {
                let total_steps = self.project.section(self.active_section).total_steps().max(1);
                if self.current_step + 1 >= total_steps {
                    // End of section reached: handle Ending stop or Auto-Fill / Intro transitions!
                    if matches!(
                        self.active_section,
                        StyleSection::EndingA | StyleSection::EndingB
                    ) {
                        self.is_playing = false;
                        self.current_step = 0;
                        self.queued_next_section = None;
                        self.stop_all_midi_notes();
                    } else {
                        if let Some(next_sec) = self.queued_next_section.take() {
                            self.active_section = next_sec;
                        } else if self.auto_fill {
                            match self.active_section {
                                StyleSection::IntroA
                                | StyleSection::FillInAA
                                | StyleSection::FillInBA => {
                                    self.active_section = StyleSection::MainA;
                                }
                                StyleSection::IntroB
                                | StyleSection::FillInBB
                                | StyleSection::FillInAB => {
                                    self.active_section = StyleSection::MainB;
                                }
                                _ => {}
                            }
                        }
                        self.current_step = 0;
                        self.last_step_instant = Instant::now();
                        self.trigger_step_midi(0);
                    }
                } else {
                    self.current_step += 1;
                    self.last_step_instant = Instant::now();
                    self.trigger_step_midi(self.current_step);
                }
            }
            ctx.request_repaint_after(std::time::Duration::from_millis(10));
        }

        self.render_menu_bar(ctx);
        self.render_top_bar(ctx);
        self.render_bottom_status_bar(ctx);
        if self.show_left_sidebar {
            self.render_left_sidebar(ctx);
        }

        egui::CentralPanel::default()
            .frame(
                egui::Frame::new()
                    .fill(egui::Color32::from_rgb(18, 20, 26))
                    .inner_margin(egui::Margin::symmetric(18, 14)),
            )
            .show(ctx, |ui| {
                // Workspace Tab Bar inside a subtle pill container
                egui::Frame::new()
                    .fill(egui::Color32::from_rgb(24, 28, 37))
                    .stroke(egui::Stroke::new(1.0, egui::Color32::from_rgb(40, 46, 60)))
                    .corner_radius(egui::CornerRadius::same(8))
                    .inner_margin(egui::Margin::symmetric(12, 8))
                    .show(ui, |ui| {
                        ui.horizontal_wrapped(|ui| {
                            ui.spacing_mut().item_spacing.x = 12.0;
                            ui.selectable_value(
                                &mut self.active_tab,
                                WorkspaceTab::DrumMachine,
                                "🥁  1. Drum Machine (Rhy1 & Rhy2)",
                            );
                            ui.selectable_value(
                                &mut self.active_tab,
                                WorkspaceTab::AccompanimentAndSlicer,
                                "🎹  2. Accompaniment & MIDI Slicer",
                            );
                            ui.selectable_value(
                                &mut self.active_tab,
                                WorkspaceTab::VoiceMixer,
                                "🎛  3. PSR-E583 Voice Mixer (890 Voices)",
                            );
                            ui.selectable_value(
                                &mut self.active_tab,
                                WorkspaceTab::CasmAndInspector,
                                "🧬  4. CASM Rules & Binary Inspector",
                            );
                        });
                    });

                ui.add_space(10.0);

                match self.active_tab {
                    WorkspaceTab::DrumMachine => self.render_drum_machine_tab(ui),
                    WorkspaceTab::AccompanimentAndSlicer => self.render_accompaniment_tab(ui),
                    WorkspaceTab::VoiceMixer => self.render_voice_mixer_tab(ui),
                    WorkspaceTab::CasmAndInspector => self.render_casm_inspector_tab(ui),
                }
            });
    }
}

impl StyleWardenApp {
    fn render_menu_bar(&mut self, ctx: &egui::Context) {
        egui::TopBottomPanel::top("top_menu_bar")
            .frame(
                egui::Frame::new()
                    .fill(egui::Color32::from_rgb(15, 17, 22))
                    .stroke(egui::Stroke::new(1.0, egui::Color32::from_rgb(34, 39, 50)))
                    .inner_margin(egui::Margin::symmetric(18, 6)),
            )
            .show(ctx, |ui| {
                egui::menu::bar(ui, |ui| {
                    ui.spacing_mut().item_spacing.x = 12.0;

                    // 1. File Menu
                    ui.menu_button("📁 File", |ui| {
                        ui.set_min_width(220.0);
                        if ui.button("📂 Open .STY or .MID File...").clicked() {
                            ui.close_menu();
                            self.import_sty_or_mid_dialog();
                        }
                        if ui.button("💾 Export PSR-E583 .STY File...").clicked() {
                            ui.close_menu();
                            self.export_sty_dialog();
                        }
                        ui.separator();
                        if ui.button("✨ Reset to Default Pop/Rock Project").clicked() {
                            ui.close_menu();
                            self.stop_all_midi_notes();
                            self.project = StyleProject::default();
                            self.sync_midi_channel_configs();
                            self.mark_dirty();
                            self.status_banner = "Reset project to default PSR-E583 Pop/Rock.".to_string();
                        }
                    });

                    // 2. Genre Presets Menu
                    ui.menu_button("🎵 Genre Presets", |ui| {
                        ui.set_min_width(250.0);
                        ui.label(
                            egui::RichText::new("Load Full-Band PSR-E583 Style")
                                .small()
                                .color(egui::Color32::GRAY),
                        );
                        ui.separator();
                        for &preset in StylePreset::ALL {
                            if ui.button(preset.label()).clicked() {
                                ui.close_menu();
                                self.stop_all_midi_notes();
                                self.project.apply_preset(preset);
                                self.sync_midi_channel_configs();
                                self.mark_dirty();
                                self.status_banner = format!("Loaded preset: {}", preset.label());
                            }
                        }
                    });

                    // 3. Time Signature / Meter Menu
                    ui.menu_button(
                        format!(
                            "🕒 Meter ({}/{})",
                            self.project.time_sig_num, self.project.time_sig_den
                        ),
                        |ui| {
                            ui.set_min_width(240.0);
                            ui.label(
                                egui::RichText::new("Quick Time Signature Presets")
                                    .small()
                                    .color(egui::Color32::GRAY),
                            );
                            let mut ts_num = self.project.time_sig_num;
                            let mut ts_den = self.project.time_sig_den;
                            let mut ts_changed = false;

                            for &(n, d, desc) in &[
                                (4u8, 4u8, "4/4 Common (16 steps/bar)"),
                                (3, 4, "3/4 Waltz (12 steps/bar)"),
                                (2, 4, "2/4 March / Polka (8 steps/bar)"),
                                (6, 8, "6/8 Compound (12 steps/bar)"),
                                (12, 8, "12/8 Slow Blues (24 steps/bar)"),
                                (5, 4, "5/4 Take Five (20 steps/bar)"),
                                (7, 8, "7/8 Odd Meter (14 steps/bar)"),
                                (9, 8, "9/8 Compound (18 steps/bar)"),
                            ] {
                                if ui
                                    .selectable_label(
                                        self.project.time_sig_num == n
                                            && self.project.time_sig_den == d,
                                        desc,
                                    )
                                    .clicked()
                                {
                                    ts_num = n;
                                    ts_den = d;
                                    ts_changed = true;
                                    ui.close_menu();
                                }
                            }

                            ui.separator();
                            ui.label(
                                egui::RichText::new("Custom Meter (Numerator / Denominator)")
                                    .small()
                                    .color(egui::Color32::GRAY),
                            );
                            ui.horizontal(|ui| {
                                ui.label("Beats:");
                                if ui
                                    .add(egui::DragValue::new(&mut ts_num).range(1..=16))
                                    .changed()
                                {
                                    ts_changed = true;
                                }
                                ui.label("/");
                                for &d in &[2u8, 4, 8, 16] {
                                    if ui
                                        .selectable_value(&mut ts_den, d, format!("{}", d))
                                        .changed()
                                    {
                                        ts_changed = true;
                                    }
                                }
                            });

                            if ts_changed {
                                self.project.set_time_signature(ts_num, ts_den);
                                self.current_step = 0;
                                self.mark_dirty();
                                self.status_banner = format!(
                                    "Set Time Signature to {}/{} ({} 16th-note steps per bar)",
                                    self.project.time_sig_num,
                                    self.project.time_sig_den,
                                    self.project.steps_per_bar()
                                );
                            }
                        },
                    );

                    // 4. USB-MIDI & Hardware Menu
                    ui.menu_button("🔌 USB-MIDI Hardware", |ui| {
                        ui.set_min_width(290.0);
                        if ui.button("⚡ Auto-Connect PSR-E583 (In + Out)").clicked() {
                            self.refresh_midi_ports();
                            ui.close_menu();
                        }
                        if ui.button("🔄 Rescan USB-MIDI Ports").clicked() {
                            self.refresh_midi_ports();
                        }
                        ui.separator();

                        ui.label(
                            egui::RichText::new("🔊 USB-MIDI Out (PSR-E583 Tone Generator)")
                                .strong(),
                        );
                        if ui
                            .selectable_label(self.selected_midi_port.is_none(), "Disconnected")
                            .clicked()
                        {
                            self.stop_all_midi_notes();
                            self.midi_conn = None;
                            self.selected_midi_port = None;
                        }
                        let out_names = self.midi_port_names.clone();
                        for (idx, name) in out_names.iter().enumerate() {
                            if ui
                                .selectable_label(self.selected_midi_port == Some(idx), name)
                                .clicked()
                            {
                                self.connect_midi_port(idx);
                            }
                        }

                        ui.separator();
                        ui.label(
                            egui::RichText::new("🎹 USB-MIDI In (PSR-E583 Left-Hand Keys)")
                                .strong(),
                        );
                        if ui
                            .selectable_label(self.selected_midi_in_port.is_none(), "Disconnected")
                            .clicked()
                        {
                            self.midi_in_conn = None;
                            self.selected_midi_in_port = None;
                        }
                        let in_names = self.midi_in_port_names.clone();
                        for (idx, name) in in_names.iter().enumerate() {
                            if ui
                                .selectable_label(self.selected_midi_in_port == Some(idx), name)
                                .clicked()
                            {
                                self.connect_midi_in_port(idx);
                            }
                        }

                        ui.separator();
                        if let Ok(mut st) = self.live_midi_in.lock() {
                            let sp = st.split_point;
                            let sp_name = format!(
                                "{}{}",
                                PITCH_CLASS_NAMES[(sp % 12) as usize],
                                (sp as i32 / 12) - 1
                            );
                            ui.horizontal(|ui| {
                                ui.label(format!("Left-Hand Split ({}):", sp_name));
                                ui.add(
                                    egui::DragValue::new(&mut st.split_point)
                                        .range(36..=72)
                                        .prefix("MIDI #"),
                                );
                            });
                        }
                    });

                    // 5. View Menu
                    ui.menu_button("📐 View", |ui| {
                        ui.set_min_width(220.0);
                        ui.checkbox(
                            &mut self.show_left_sidebar,
                            "Show Left Sidebar (Sections & Arranger)",
                        );
                        ui.separator();
                        ui.selectable_value(
                            &mut self.active_tab,
                            WorkspaceTab::DrumMachine,
                            "🥁 1. Drum Machine",
                        );
                        ui.selectable_value(
                            &mut self.active_tab,
                            WorkspaceTab::AccompanimentAndSlicer,
                            "🎹 2. Accompaniment & Slicer",
                        );
                        ui.selectable_value(
                            &mut self.active_tab,
                            WorkspaceTab::VoiceMixer,
                            "🎛 3. Voice Mixer",
                        );
                        ui.selectable_value(
                            &mut self.active_tab,
                            WorkspaceTab::CasmAndInspector,
                            "🧬 4. CASM & Binary Inspector",
                        );
                    });

                    ui.separator();
                    if ui
                        .selectable_label(
                            self.show_left_sidebar,
                            if self.show_left_sidebar {
                                "◧ Hide Sidebar"
                            } else {
                                "◧ Show Sidebar"
                            },
                        )
                        .clicked()
                    {
                        self.show_left_sidebar = !self.show_left_sidebar;
                    }

                    // Compact USB-C Hardware Status Pill
                    let usb_connected = self.midi_conn.is_some() || self.midi_in_conn.is_some();
                    let status_text = match (self.midi_conn.is_some(), self.midi_in_conn.is_some()) {
                        (true, true) => "🟢 USB-C In+Out Connected",
                        (true, false) => "🟢 USB-C Out Connected",
                        (false, true) => "🟢 USB-C In Connected",
                        (false, false) => "⚪ USB-MIDI Disconnected",
                    };
                    ui.label(
                        egui::RichText::new(status_text).small().color(if usb_connected {
                            egui::Color32::from_rgb(100, 225, 145)
                        } else {
                            egui::Color32::GRAY
                        }),
                    );
                });
            });
    }

    fn render_top_bar(&mut self, ctx: &egui::Context) {
        egui::TopBottomPanel::top("top_transport_bar")
            .frame(
                egui::Frame::new()
                    .fill(egui::Color32::from_rgb(22, 25, 33))
                    .stroke(egui::Stroke::new(1.0, egui::Color32::from_rgb(38, 44, 58)))
                    .inner_margin(egui::Margin::symmetric(18, 10)),
            )
            .show(ctx, |ui| {
                ui.horizontal_wrapped(|ui| {
                    ui.spacing_mut().item_spacing.x = 12.0;

                    // Brand Logo
                    ui.heading(
                        egui::RichText::new("STYLEWARDEN")
                            .strong()
                            .color(egui::Color32::from_rgb(95, 205, 255)),
                    );
                    ui.separator();

                    // Style Name
                    ui.label(egui::RichText::new("Style:").color(egui::Color32::LIGHT_GRAY));
                    if ui
                        .add(
                            egui::TextEdit::singleline(&mut self.project.style_name)
                                .desired_width(140.0),
                        )
                        .changed()
                    {
                        self.mark_dirty();
                    }

                    // Tempo BPM
                    ui.label(egui::RichText::new("Tempo:").color(egui::Color32::LIGHT_GRAY));
                    if ui
                        .add(
                            egui::DragValue::new(&mut self.project.bpm)
                                .range(40.0..=240.0)
                                .speed(0.5)
                                .suffix(" BPM"),
                        )
                        .changed()
                    {
                        self.mark_dirty();
                    }

                    // Time Signature Badge (Clean inline controls)
                    ui.label(egui::RichText::new("Meter:").color(egui::Color32::LIGHT_GRAY));
                    let mut ts_num = self.project.time_sig_num;
                    let mut ts_den = self.project.time_sig_den;
                    let mut ts_changed = false;
                    if ui
                        .add(egui::DragValue::new(&mut ts_num).range(1..=16))
                        .on_hover_text("Beats per bar (Numerator)")
                        .changed()
                    {
                        ts_changed = true;
                    }
                    ui.label("/");
                    egui::ComboBox::from_id_salt("header_ts_den")
                        .selected_text(format!("{}", ts_den))
                        .width(48.0)
                        .show_ui(ui, |ui| {
                            for &d in &[2u8, 4, 8, 16] {
                                if ui.selectable_value(&mut ts_den, d, format!("{}", d)).changed() {
                                    ts_changed = true;
                                }
                            }
                        });
                    if ts_changed {
                        self.project.set_time_signature(ts_num, ts_den);
                        self.current_step = 0;
                        self.mark_dirty();
                    }

                    ui.separator();

                    // Transport Play / Stop Button
                    let play_label = if self.is_playing {
                        "⏹  Stop"
                    } else {
                        "▶  Play Style"
                    };
                    let play_btn = egui::Button::new(
                        egui::RichText::new(play_label).strong().color(if self.is_playing {
                            egui::Color32::from_rgb(255, 235, 235)
                        } else {
                            egui::Color32::from_rgb(15, 30, 20)
                        }),
                    )
                    .fill(if self.is_playing {
                        egui::Color32::from_rgb(195, 55, 60)
                    } else {
                        egui::Color32::from_rgb(90, 220, 140)
                    })
                    .min_size(egui::vec2(110.0, 30.0));

                    if ui.add(play_btn).clicked() {
                        self.is_playing = !self.is_playing;
                        self.current_step = 0;
                        self.last_step_instant = Instant::now();
                        if self.is_playing {
                            if self.midi_conn.is_none() || self.midi_in_conn.is_none() {
                                self.refresh_midi_ports();
                            }
                            self.sync_midi_channel_configs();
                            self.trigger_step_midi(0);
                        } else {
                            self.queued_next_section = None;
                            self.stop_all_midi_notes();
                        }
                    }

                    // Digital Bar:Step & Active Section Readout
                    let spb = self.project.steps_per_bar().max(1);
                    egui::Frame::new()
                        .fill(egui::Color32::from_rgb(14, 16, 22))
                        .stroke(egui::Stroke::new(1.0, egui::Color32::from_rgb(42, 48, 62)))
                        .corner_radius(egui::CornerRadius::same(6))
                        .inner_margin(egui::Margin::symmetric(12, 5))
                        .show(ui, |ui| {
                            ui.label(
                                egui::RichText::new(format!(
                                    "{}  |  Bar {} : Step {:02}",
                                    self.active_section.as_marker_str(),
                                    (self.current_step / spb) + 1,
                                    (self.current_step % spb) + 1
                                ))
                                .monospace()
                                .strong()
                                .color(egui::Color32::from_rgb(250, 215, 95)),
                            );
                        });

                    // Live Chord Readout Pill
                    let chord_badge = if self.acmp_on {
                        format!("🎹 {}", self.current_chord.display_name())
                    } else {
                        "🎹 C Maj7 (ACMP Off)".to_string()
                    };
                    egui::Frame::new()
                        .fill(if self.acmp_on {
                            egui::Color32::from_rgb(48, 42, 24)
                        } else {
                            egui::Color32::from_rgb(30, 33, 42)
                        })
                        .stroke(egui::Stroke::new(
                            1.0,
                            if self.acmp_on {
                                egui::Color32::from_rgb(215, 170, 60)
                            } else {
                                egui::Color32::from_rgb(60, 65, 80)
                            },
                        ))
                        .corner_radius(egui::CornerRadius::same(6))
                        .inner_margin(egui::Margin::symmetric(10, 5))
                        .show(ui, |ui| {
                            ui.label(
                                egui::RichText::new(chord_badge)
                                    .strong()
                                    .color(if self.acmp_on {
                                        egui::Color32::from_rgb(255, 215, 95)
                                    } else {
                                        egui::Color32::GRAY
                                    }),
                            );
                        });

                    ui.separator();

                    // Primary Export Action Button (clean inline flow, never overlaps)
                    let export_btn = egui::Button::new(
                        egui::RichText::new("💾 Export .STY")
                            .strong()
                            .color(egui::Color32::from_rgb(15, 25, 20)),
                    )
                    .fill(egui::Color32::from_rgb(90, 210, 145))
                    .min_size(egui::vec2(120.0, 30.0));
                    if ui.add(export_btn).clicked() {
                        self.export_sty_dialog();
                    }
                });
            });
    }

    fn render_left_sidebar(&mut self, ctx: &egui::Context) {
        egui::SidePanel::left("studio_left_sidebar")
            .resizable(true)
            .default_width(285.0)
            .min_width(255.0)
            .max_width(350.0)
            .frame(
                egui::Frame::new()
                    .fill(egui::Color32::from_rgb(20, 23, 30))
                    .stroke(egui::Stroke::new(1.0, egui::Color32::from_rgb(36, 42, 55)))
                    .inner_margin(egui::Margin::symmetric(14, 14)),
            )
            .show(ctx, |ui| {
                egui::ScrollArea::vertical()
                    .id_salt("left_sidebar_scroll")
                    .auto_shrink([false, false])
                    .show(ui, |ui| {
                        let mut mark_dirty = false;

                        // CARD 1: SFF1 Style Sections & Active Section Controls
                        studio_card_frame().show(ui, |ui| {
                            ui.label(
                                egui::RichText::new("📑 SFF1 Style Sections")
                                    .strong()
                                    .size(14.5),
                            );
                            ui.add_space(4.0);

                            let spb = self.project.steps_per_bar().max(1);
                            egui::Grid::new("sidebar_sections_grid")
                                .num_columns(2)
                                .spacing([8.0, 8.0])
                                .show(ui, |ui| {
                                    for (idx, &sec) in StyleSection::ALL.iter().enumerate() {
                                        let is_active = self.active_section == sec;
                                        let is_queued = self.queued_next_section == Some(sec);
                                        let (enabled, bars) = {
                                            let pat = self.project.section(sec);
                                            (pat.enabled, pat.bars)
                                        };
                                        let label_text = if is_queued {
                                            format!("⏭ {} ({}b)", sec.as_marker_str(), bars)
                                        } else {
                                            format!("{} ({}b)", sec.as_marker_str(), bars)
                                        };
                                        let fill_color = if is_active {
                                            egui::Color32::from_rgb(55, 120, 210)
                                        } else if is_queued {
                                            egui::Color32::from_rgb(195, 130, 40)
                                        } else if enabled {
                                            egui::Color32::from_rgb(36, 42, 55)
                                        } else {
                                            egui::Color32::from_rgb(22, 25, 32)
                                        };

                                        let btn = egui::Button::new(
                                            egui::RichText::new(label_text).small().strong().color(
                                                if is_active || is_queued || enabled {
                                                    egui::Color32::WHITE
                                                } else {
                                                    egui::Color32::DARK_GRAY
                                                },
                                            ),
                                        )
                                        .fill(fill_color)
                                        .min_size(egui::vec2(114.0, 28.0));

                                        if ui.add(btn).clicked() {
                                            self.stop_all_midi_notes();
                                            if self.is_playing && self.auto_fill {
                                                let step_in_bar = self.current_step % spb;
                                                match (self.active_section, sec) {
                                                    (StyleSection::MainA, StyleSection::MainB) => {
                                                        self.active_section = StyleSection::FillInAB;
                                                        self.queued_next_section =
                                                            Some(StyleSection::MainB);
                                                        self.current_step = step_in_bar.min(
                                                            self.project
                                                                .section(StyleSection::FillInAB)
                                                                .total_steps()
                                                                - 1,
                                                        );
                                                    }
                                                    (StyleSection::MainB, StyleSection::MainA) => {
                                                        self.active_section = StyleSection::FillInBA;
                                                        self.queued_next_section =
                                                            Some(StyleSection::MainA);
                                                        self.current_step = step_in_bar.min(
                                                            self.project
                                                                .section(StyleSection::FillInBA)
                                                                .total_steps()
                                                                - 1,
                                                        );
                                                    }
                                                    (StyleSection::MainA, StyleSection::MainA) => {
                                                        self.active_section = StyleSection::FillInAA;
                                                        self.queued_next_section =
                                                            Some(StyleSection::MainA);
                                                        self.current_step = step_in_bar.min(
                                                            self.project
                                                                .section(StyleSection::FillInAA)
                                                                .total_steps()
                                                                - 1,
                                                        );
                                                    }
                                                    (StyleSection::MainB, StyleSection::MainB) => {
                                                        self.active_section = StyleSection::FillInBB;
                                                        self.queued_next_section =
                                                            Some(StyleSection::MainB);
                                                        self.current_step = step_in_bar.min(
                                                            self.project
                                                                .section(StyleSection::FillInBB)
                                                                .total_steps()
                                                                - 1,
                                                        );
                                                    }
                                                    (_, StyleSection::IntroA) => {
                                                        self.active_section = StyleSection::IntroA;
                                                        self.queued_next_section =
                                                            Some(StyleSection::MainA);
                                                        self.current_step = 0;
                                                    }
                                                    (_, StyleSection::IntroB) => {
                                                        self.active_section = StyleSection::IntroB;
                                                        self.queued_next_section =
                                                            Some(StyleSection::MainB);
                                                        self.current_step = 0;
                                                    }
                                                    _ => {
                                                        self.active_section = sec;
                                                        self.queued_next_section = None;
                                                        self.current_step = 0;
                                                    }
                                                }
                                            } else {
                                                self.active_section = sec;
                                                self.queued_next_section = None;
                                                self.current_step = 0;
                                            }
                                        }
                                        if idx % 2 == 1 {
                                            ui.end_row();
                                        }
                                    }
                                });

                            ui.add_space(6.0);
                            ui.separator();
                            ui.add_space(4.0);

                            // Active Section Properties
                            let sec_pat = self.project.section_mut(self.active_section);
                            ui.horizontal(|ui| {
                                if ui
                                    .checkbox(&mut sec_pat.enabled, "Include in .STY")
                                    .changed()
                                {
                                    mark_dirty = true;
                                }
                                ui.with_layout(
                                    egui::Layout::right_to_left(egui::Align::Center),
                                    |ui| {
                                        let mut bars = sec_pat.bars;
                                        if ui
                                            .add(
                                                egui::DragValue::new(&mut bars)
                                                    .range(1..=8)
                                                    .suffix(" bar(s)"),
                                            )
                                            .changed()
                                        {
                                            sec_pat.set_bars(bars);
                                            mark_dirty = true;
                                        }
                                        ui.label("Length:");
                                    },
                                );
                            });

                            ui.add_space(4.0);
                            ui.horizontal(|ui| {
                                if ui.button("📋 Copy").clicked() {
                                    self.copied_section =
                                        Some(self.project.section(self.active_section).clone());
                                    self.status_banner = format!(
                                        "Copied section '{}'",
                                        self.active_section.as_marker_str()
                                    );
                                }
                                if let Some(copied) = self.copied_section.clone() {
                                    if ui.button("📥 Paste").clicked() {
                                        let target = self.project.section_mut(self.active_section);
                                        target.bars = copied.bars;
                                        target.drum_rows = copied.drum_rows;
                                        target.accompaniment_notes = copied.accompaniment_notes;
                                        mark_dirty = true;
                                        self.status_banner = format!(
                                            "Pasted pattern into '{}'",
                                            self.active_section.as_marker_str()
                                        );
                                    }
                                }
                                if ui.button("🧹 Clear Drums").clicked() {
                                    self.project.section_mut(self.active_section).clear_drums();
                                    mark_dirty = true;
                                }
                            });
                        });

                        ui.add_space(12.0);

                        // CARD 2: Live USB-C Arranger (MIDI In Chord Recognition)
                        studio_card_frame().show(ui, |ui| {
                            ui.label(
                                egui::RichText::new("🎹 Live USB-C Arranger")
                                    .strong()
                                    .size(14.5),
                            );
                            ui.label(
                                egui::RichText::new(
                                    "Play chords with your left hand on the PSR-E583 over USB-C.",
                                )
                                .small()
                                .color(egui::Color32::GRAY),
                            );
                            ui.add_space(6.0);

                            // Prominent Chord Display Box
                            egui::Frame::new()
                                .fill(egui::Color32::from_rgb(16, 19, 26))
                                .stroke(egui::Stroke::new(
                                    1.0,
                                    if self.acmp_on {
                                        egui::Color32::from_rgb(220, 175, 65)
                                    } else {
                                        egui::Color32::from_rgb(55, 60, 75)
                                    },
                                ))
                                .corner_radius(egui::CornerRadius::same(6))
                                .inner_margin(egui::Margin::symmetric(12, 8))
                                .show(ui, |ui| {
                                    ui.horizontal(|ui| {
                                        ui.label(
                                            egui::RichText::new("Active Chord:")
                                                .small()
                                                .color(egui::Color32::GRAY),
                                        );
                                        ui.with_layout(
                                            egui::Layout::right_to_left(egui::Align::Center),
                                            |ui| {
                                                let txt = if self.acmp_on {
                                                    self.current_chord.display_name()
                                                } else {
                                                    "C Maj7 (Bypass)".to_string()
                                                };
                                                ui.label(
                                                    egui::RichText::new(txt)
                                                        .size(16.0)
                                                        .strong()
                                                        .color(if self.acmp_on {
                                                            egui::Color32::from_rgb(255, 215, 90)
                                                        } else {
                                                            egui::Color32::GRAY
                                                        }),
                                                );
                                            },
                                        );
                                    });
                                });

                            ui.add_space(6.0);

                            if ui
                                .checkbox(&mut self.acmp_on, "ACMP On (Live CASM Transpose)")
                                .changed()
                            {
                                self.revoice_held_accompaniment_notes();
                            }
                            ui.checkbox(&mut self.sync_start, "Sync Start (Left-Hand Trigger)");
                            ui.checkbox(&mut self.auto_fill, "Auto-Fill Section Transitions");

                            ui.add_space(4.0);
                            ui.separator();

                            // Collapsible Chord Audition Pad (hides the 19 buttons unless needed)
                            egui::CollapsingHeader::new(
                                egui::RichText::new("🎼 Chord Audition Pad").strong(),
                            )
                            .default_open(false)
                            .show(ui, |ui| {
                                ui.add_space(4.0);
                                let mut manual_chord_changed = false;
                                ui.label(
                                    egui::RichText::new("Root Note:")
                                        .small()
                                        .color(egui::Color32::GRAY),
                                );
                                egui::Grid::new("sidebar_chord_roots")
                                    .num_columns(6)
                                    .spacing([4.0, 4.0])
                                    .show(ui, |ui| {
                                        for (pc, &name) in PITCH_CLASS_NAMES.iter().enumerate() {
                                            let is_root = self.current_chord.root as usize == pc;
                                            if ui
                                                .add_sized(
                                                    [34.0, 24.0],
                                                    egui::Button::new(
                                                        egui::RichText::new(name).small(),
                                                    )
                                                    .fill(if is_root {
                                                        egui::Color32::from_rgb(55, 120, 210)
                                                    } else {
                                                        egui::Color32::from_rgb(34, 39, 50)
                                                    }),
                                                )
                                                .clicked()
                                            {
                                                self.current_chord.root = pc as u8;
                                                manual_chord_changed = true;
                                            }
                                            if pc % 6 == 5 {
                                                ui.end_row();
                                            }
                                        }
                                    });

                                ui.add_space(4.0);
                                ui.label(
                                    egui::RichText::new("Chord Type:")
                                        .small()
                                        .color(egui::Color32::GRAY),
                                );
                                ui.horizontal_wrapped(|ui| {
                                    ui.spacing_mut().item_spacing = egui::vec2(4.0, 4.0);
                                    for &q in ChordQuality::ALL {
                                        let lbl = if q == ChordQuality::Major {
                                            "Maj"
                                        } else {
                                            q.suffix()
                                        };
                                        if ui
                                            .selectable_label(self.current_chord.quality == q, lbl)
                                            .clicked()
                                        {
                                            self.current_chord.quality = q;
                                            manual_chord_changed = true;
                                        }
                                    }
                                });

                                if manual_chord_changed {
                                    if let Ok(mut st) = self.live_midi_in.lock() {
                                        st.detected_chord = Some(self.current_chord);
                                    }
                                    self.revoice_held_accompaniment_notes();
                                }
                            });
                        });

                        if mark_dirty {
                            self.mark_dirty();
                        }
                    });
            });
    }

    fn render_bottom_status_bar(&mut self, ctx: &egui::Context) {
        self.ensure_compiled();
        egui::TopBottomPanel::bottom("status_footer_bar")
            .frame(
                egui::Frame::new()
                    .fill(egui::Color32::from_rgb(15, 17, 22))
                    .stroke(egui::Stroke::new(1.0, egui::Color32::from_rgb(34, 39, 50)))
                    .inner_margin(egui::Margin::symmetric(18, 7)),
            )
            .show(ctx, |ui| {
                ui.horizontal(|ui| {
                    let all_ok = self.cached_report.all_passed();
                    let badge_color = if all_ok {
                        egui::Color32::from_rgb(90, 220, 135)
                    } else {
                        egui::Color32::from_rgb(255, 110, 110)
                    };
                    ui.label(
                        egui::RichText::new(if all_ok {
                            "✔ PSR-E583 SFF1 VALID (10/10 Checks)"
                        } else {
                            "⚠ SFF1 Validation Warning"
                        })
                        .strong()
                        .color(badge_color),
                    );
                    ui.separator();
                    ui.label(
                        egui::RichText::new(format!(
                            "Size: {} B (SMF0: {} B + CASM: {} B)",
                            self.cached_report.total_bytes,
                            self.cached_report.smf_bytes,
                            self.cached_report.casm_bytes
                        ))
                        .monospace()
                        .small(),
                    );
                    ui.separator();
                    ui.label(
                        egui::RichText::new(&self.status_banner)
                            .small()
                            .color(egui::Color32::LIGHT_GRAY),
                    );
                });
            });
    }

    fn render_drum_machine_tab(&mut self, ui: &mut egui::Ui) {
        let mut mark_dirty = false;

        // Top Toolbar Card: Drum Kits, Velocity Brush, and Add Instrument Popover Menu
        studio_card_frame().show(ui, |ui| {
            ui.horizontal_wrapped(|ui| {
                ui.spacing_mut().item_spacing.x = 12.0;

                // Drum Kit Selectors for Rhy2 (Ch 10) & Rhy1 (Ch 9)
                for &rhy_part in &[AccompanimentPart::Rhy2, AccompanimentPart::Rhy1] {
                    let current_voice_no = self
                        .project
                        .channel_configs
                        .iter()
                        .find(|c| c.part == rhy_part)
                        .map(|c| c.voice_no)
                        .unwrap_or(353);
                    let current_name = PSR_E583_VOICES
                        .iter()
                        .find(|v| v.voice_no == current_voice_no)
                        .map(|v| format!("#{:03} {}", v.voice_no, v.name))
                        .unwrap_or_else(|| "Drum Kit".to_string());

                    ui.label(
                        egui::RichText::new(format!("{}:", rhy_part.short_label())).strong(),
                    );
                    egui::ComboBox::from_id_salt(format!("kit_combo_{:?}", rhy_part))
                        .selected_text(current_name)
                        .width(195.0)
                        .show_ui(ui, |ui| {
                            for v in PSR_E583_VOICES.iter().filter(|v| v.is_drum_kit()) {
                                let label = format!(
                                    "#{:03} {} (MSB:{} LSB:{} PC:{})",
                                    v.voice_no,
                                    v.name,
                                    v.msb.unwrap_or(127),
                                    v.lsb.unwrap_or(0),
                                    v.pc.unwrap_or(0) + 1
                                );
                                if ui
                                    .selectable_label(v.voice_no == current_voice_no, label)
                                    .clicked()
                                {
                                    self.project.set_channel_voice(rhy_part, v.voice_no);
                                    self.sync_midi_channel_configs();
                                    mark_dirty = true;
                                }
                            }
                        });
                }

                ui.separator();

                // Velocity Brush Controls
                ui.label(egui::RichText::new("Brush Velocity:").strong());
                if ui
                    .selectable_label(self.brush_velocity == 120, "Accent (120)")
                    .clicked()
                {
                    self.brush_velocity = 120;
                }
                if ui
                    .selectable_label(self.brush_velocity == 100, "Normal (100)")
                    .clicked()
                {
                    self.brush_velocity = 100;
                }
                if ui
                    .selectable_label(self.brush_velocity == 68, "Ghost (68)")
                    .clicked()
                {
                    self.brush_velocity = 68;
                }
                ui.add(egui::Slider::new(&mut self.brush_velocity, 1..=127).text("Vel"));

                ui.separator();

                // Clean Popover Menu for Adding a New PSR-E583 Drum Instrument Row
                ui.menu_button(
                    egui::RichText::new("➕ Add Drum Instrument...").strong(),
                    |ui| {
                        ui.set_min_width(360.0);
                        ui.label(
                            egui::RichText::new("Add PSR-E583 Drum / Percussion Row to Grid")
                                .strong(),
                        );
                        ui.separator();

                        ui.horizontal(|ui| {
                            ui.label("Target Channel:");
                            ui.selectable_value(
                                &mut self.selected_new_drum_part,
                                AccompanimentPart::Rhy2,
                                "Rhy2 (Ch 10 Main Kit)",
                            );
                            ui.selectable_value(
                                &mut self.selected_new_drum_part,
                                AccompanimentPart::Rhy1,
                                "Rhy1 (Ch 9 Sub-Perc)",
                            );
                        });
                        ui.separator();

                        egui::ScrollArea::vertical()
                            .max_height(280.0)
                            .show(ui, |ui| {
                                let sec_pat = self.project.section_mut(self.active_section);
                                let spb = sec_pat.steps_per_bar.max(1);
                                let bars = sec_pat.bars;
                                for (idx, d) in DRUM_KIT_NOTES.iter().enumerate() {
                                    let text = format!(
                                        "[{}] MIDI #{} ({}) — {}{}",
                                        d.group,
                                        d.midi_note,
                                        d.midi_note_name,
                                        d.standard_name,
                                        d.real_kit_name
                                            .map(|r| format!(" / Real: {}", r))
                                            .unwrap_or_default()
                                    );
                                    if ui.button(text).clicked() {
                                        self.selected_new_drum_idx = idx;
                                        sec_pat.drum_rows.push(DrumRow::new_with_steps_per_bar(
                                            d.midi_note,
                                            d.standard_name,
                                            self.selected_new_drum_part,
                                            bars,
                                            spb,
                                        ));
                                        mark_dirty = true;
                                        ui.close_menu();
                                    }
                                }
                            });
                    },
                );
            });
        });

        ui.add_space(10.0);

        // Check whether Rhy2 is currently set to Voice #353 Real Drum Kit
        let rhy2_is_real_kit = self
            .project
            .channel_configs
            .iter()
            .find(|c| c.part == AccompanimentPart::Rhy2)
            .is_some_and(|c| c.voice_no == 353);

        let sp_beat = self.project.steps_per_beat().max(1);

        // Main Step Sequencer Grid inside a Padded Card Frame
        studio_card_frame().show(ui, |ui| {
            egui::ScrollArea::both()
                .id_salt("drum_grid_scroll")
                .auto_shrink([false, false])
                .show(ui, |ui| {
                    let sec_pat = self.project.section_mut(self.active_section);
                    let spb = sec_pat.steps_per_bar.max(1);
                    let total_steps = sec_pat.total_steps();

                    // Header Row: Step numbers & bar markers
                    ui.horizontal(|ui| {
                        ui.spacing_mut().item_spacing.x = 5.0;
                        ui.add_sized(
                            [265.0, 24.0],
                            egui::Label::new(
                                egui::RichText::new("Kit Piece / MIDI Note")
                                    .strong()
                                    .color(egui::Color32::LIGHT_GRAY),
                            ),
                        );
                        for step in 0..total_steps {
                            let is_downbeat = (step % spb).is_multiple_of(sp_beat);
                            let is_playhead = self.is_playing && self.current_step == step;
                            let lbl = if step.is_multiple_of(spb) {
                                format!("B{}", (step / spb) + 1)
                            } else {
                                format!("{}", (step % spb) + 1)
                            };
                            let col = if is_playhead {
                                egui::Color32::from_rgb(90, 230, 140)
                            } else if is_downbeat {
                                egui::Color32::from_rgb(245, 205, 85)
                            } else {
                                egui::Color32::GRAY
                            };
                            ui.add_sized(
                                [28.0, 24.0],
                                egui::Label::new(
                                    egui::RichText::new(lbl).small().strong().color(col),
                                ),
                            );
                            if (step % spb) % sp_beat == sp_beat - 1 || step % spb == spb - 1 {
                                ui.add_space(6.0);
                            }
                        }
                    });

                    ui.add_space(4.0);
                    ui.separator();
                    ui.add_space(6.0);

                    let mut row_to_delete: Option<usize> = None;

                    for (row_idx, row) in sec_pat.drum_rows.iter_mut().enumerate() {
                        ui.horizontal(|ui| {
                            ui.spacing_mut().item_spacing.x = 5.0;

                            // Mute button
                            let mute_btn = egui::Button::new(
                                egui::RichText::new(if row.muted { "M" } else { "🔊" }).small(),
                            )
                            .fill(if row.muted {
                                egui::Color32::from_rgb(170, 50, 50)
                            } else {
                                egui::Color32::from_rgb(38, 46, 58)
                            });
                            if ui.add_sized([28.0, 28.0], mute_btn).clicked() {
                                row.muted = !row.muted;
                                mark_dirty = true;
                            }

                            // Channel badge (Rhy2 Ch10 vs Rhy1 Ch9)
                            let ch_badge_color = if row.part == AccompanimentPart::Rhy2 {
                                egui::Color32::from_rgb(50, 115, 190)
                            } else {
                                egui::Color32::from_rgb(155, 85, 185)
                            };
                            if ui
                                .add_sized(
                                    [48.0, 28.0],
                                    egui::Button::new(
                                        egui::RichText::new(
                                            if row.part == AccompanimentPart::Rhy2 {
                                                "Rhy2"
                                            } else {
                                                "Rhy1"
                                            },
                                        )
                                        .small()
                                        .strong(),
                                    )
                                    .fill(ch_badge_color),
                                )
                                .on_hover_text(
                                    "Click to toggle between Rhy2 (Ch 10) and Rhy1 (Ch 9)",
                                )
                                .clicked()
                            {
                                row.part = if row.part == AccompanimentPart::Rhy2 {
                                    AccompanimentPart::Rhy1
                                } else {
                                    AccompanimentPart::Rhy2
                                };
                                mark_dirty = true;
                            }

                            // Instrument Name & Note Badge
                            let note_meta =
                                DRUM_KIT_NOTES.iter().find(|d| d.midi_note == row.midi_note);
                            let display_name = if rhy2_is_real_kit
                                && row.part == AccompanimentPart::Rhy2
                                && let Some(meta) = note_meta
                                && let Some(real_name) = meta.real_kit_name
                            {
                                format!("{} (#{})", real_name, row.midi_note)
                            } else {
                                format!("{} (#{})", row.name, row.midi_note)
                            };

                            ui.add_sized(
                                [155.0, 28.0],
                                egui::Label::new(
                                    egui::RichText::new(display_name).color(if row.muted {
                                        egui::Color32::DARK_GRAY
                                    } else {
                                        egui::Color32::WHITE
                                    }),
                                )
                                .truncate(),
                            );

                            if ui
                                .add_sized([22.0, 28.0], egui::Button::new("×").small())
                                .on_hover_text("Remove Drum Row")
                                .clicked()
                            {
                                row_to_delete = Some(row_idx);
                            }

                            // 16th-Note Step Pads
                            for step in 0..total_steps {
                                let vel = *row.steps.get(step).unwrap_or(&0);
                                let is_active = vel > 0;
                                let is_downbeat = (step % spb).is_multiple_of(sp_beat);
                                let is_playhead = self.is_playing && self.current_step == step;

                                let pad_color = if is_active {
                                    let intensity = (vel as f32 / 127.0).clamp(0.35, 1.0);
                                    if row.part == AccompanimentPart::Rhy2 {
                                        egui::Color32::from_rgb(
                                            (55.0 + 185.0 * intensity) as u8,
                                            (130.0 + 95.0 * intensity) as u8,
                                            45,
                                        )
                                    } else {
                                        egui::Color32::from_rgb(
                                            (95.0 + 145.0 * intensity) as u8,
                                            70,
                                            (140.0 + 110.0 * intensity) as u8,
                                        )
                                    }
                                } else if is_playhead {
                                    egui::Color32::from_rgb(55, 78, 65)
                                } else if is_downbeat {
                                    egui::Color32::from_rgb(44, 49, 62)
                                } else {
                                    egui::Color32::from_rgb(31, 34, 43)
                                };

                                let pad_text = if is_active {
                                    format!("{}", vel)
                                } else {
                                    "·".to_string()
                                };

                                let (rect, resp) = ui.allocate_exact_size(
                                    egui::vec2(28.0, 28.0),
                                    egui::Sense::click_and_drag(),
                                );
                                if ui.is_rect_visible(rect) {
                                    let stroke = if is_playhead {
                                        egui::Stroke::new(
                                            1.5,
                                            egui::Color32::from_rgb(100, 245, 150),
                                        )
                                    } else {
                                        egui::Stroke::new(
                                            1.0,
                                            egui::Color32::from_rgb(18, 20, 26),
                                        )
                                    };
                                    ui.painter().rect(
                                        rect,
                                        5.0,
                                        pad_color,
                                        stroke,
                                        egui::StrokeKind::Inside,
                                    );
                                    ui.painter().text(
                                        rect.center(),
                                        egui::Align2::CENTER_CENTER,
                                        pad_text,
                                        egui::FontId::monospace(10.5),
                                        if is_active {
                                            egui::Color32::BLACK
                                        } else {
                                            egui::Color32::from_gray(100)
                                        },
                                    );
                                }

                                if resp.clicked() {
                                    row.steps[step] =
                                        if is_active { 0 } else { self.brush_velocity };
                                    mark_dirty = true;
                                } else if resp.secondary_clicked()
                                    || (resp.dragged() && ui.input(|i| i.pointer.secondary_down()))
                                {
                                    if row.steps[step] != 0 {
                                        row.steps[step] = 0;
                                        mark_dirty = true;
                                    }
                                } else if resp.dragged() && ui.input(|i| i.pointer.primary_down()) {
                                    if row.steps[step] != self.brush_velocity {
                                        row.steps[step] = self.brush_velocity;
                                        mark_dirty = true;
                                    }
                                }

                                if (step % spb) % sp_beat == sp_beat - 1 || step % spb == spb - 1 {
                                    ui.add_space(6.0);
                                }
                            }
                        });
                        ui.add_space(4.0);
                    }

                    if let Some(del_idx) = row_to_delete {
                        sec_pat.drum_rows.remove(del_idx);
                        mark_dirty = true;
                    }
                });
        });

        if mark_dirty {
            self.mark_dirty();
        }
    }

    fn render_accompaniment_tab(&mut self, ui: &mut egui::Ui) {
        let mut mark_dirty = false;

        ui.columns(2, |cols| {
            // Left Column: C-Maj7 Accompaniment Step Sequencer & Note Inspector (Ch 11–16)
            studio_card_frame().show(&mut cols[0], |ui| {
                ui.heading("🎹 C-Maj7 Accompaniment Tracks (Channels 11–16)");
                ui.add_space(4.0);
                ui.label(
                    egui::RichText::new(
                        "All accompaniment tracks are composed in C Major 7 (C, E, G, B). The PSR-E583 CASM engine transposes them in real time to match your left-hand chords.",
                    )
                    .small()
                    .color(egui::Color32::GRAY),
                );
                ui.add_space(6.0);
                ui.separator();
                ui.add_space(6.0);

                ui.horizontal_wrapped(|ui| {
                    for &part in &[
                        AccompanimentPart::Bass,
                        AccompanimentPart::Chd1,
                        AccompanimentPart::Chd2,
                        AccompanimentPart::Pad,
                        AccompanimentPart::Phr1,
                        AccompanimentPart::Phr2,
                    ] {
                        let count = self
                            .project
                            .section(self.active_section)
                            .accompaniment_notes
                            .iter()
                            .filter(|n| n.channel == part.midi_channel())
                            .count();
                        let label = format!("{} ({})", part.short_label(), count);
                        ui.selectable_value(&mut self.selected_accomp_part, part, label);
                    }
                });

                ui.add_space(6.0);
                ui.separator();
                ui.add_space(6.0);

                let target_ch = self.selected_accomp_part.midi_channel();
                let tpq = self.project.ticks_per_quarter.max(96) as u32;
                let step_ticks = tpq / 4;
                let sp_beat = self.project.steps_per_beat().max(1);

                // Interactive C-Maj7 Step Grid for the selected Accompaniment Part
                ui.label(
                    egui::RichText::new(format!(
                        "16th-Note C-Maj7 Step Matrix — {} in {}",
                        self.selected_accomp_part.short_label(),
                        self.active_section.as_marker_str()
                    ))
                    .strong(),
                );
                ui.add_space(6.0);

                let candidate_pitches: &[(u8, &str)] =
                    if self.selected_accomp_part == AccompanimentPart::Bass {
                        &[
                            (48, "C3 (Octave)"),
                            (47, "B2 (Maj7)"),
                            (43, "G2 (5th)"),
                            (40, "E2 (3rd)"),
                            (36, "C2 (Root)"),
                        ]
                    } else {
                        &[
                            (76, "E5 (3rd Hi)"),
                            (72, "C5 (Octave)"),
                            (71, "B4 (Maj7)"),
                            (67, "G4 (5th)"),
                            (64, "E4 (3rd)"),
                            (60, "C4 (Root)"),
                        ]
                    };

                let sec_pat = self.project.section_mut(self.active_section);
                let spb = sec_pat.steps_per_bar.max(1);
                let total_steps = sec_pat.total_steps();

                egui::ScrollArea::horizontal()
                    .id_salt("accomp_step_grid")
                    .show(ui, |ui| {
                        for &(pitch, pitch_label) in candidate_pitches {
                            ui.horizontal(|ui| {
                                ui.spacing_mut().item_spacing.x = 4.0;
                                ui.add_sized(
                                    [100.0, 26.0],
                                    egui::Label::new(egui::RichText::new(pitch_label).monospace()),
                                );
                                for step in 0..total_steps {
                                    let s_tick = (step as u32) * step_ticks;
                                    let existing_idx =
                                        sec_pat.accompaniment_notes.iter().position(|n| {
                                            n.channel == target_ch
                                                && n.note == pitch
                                                && n.start_tick
                                                    >= s_tick.saturating_sub(step_ticks / 3)
                                                && n.start_tick < s_tick + (step_ticks * 2 / 3)
                                        });
                                    let is_on = existing_idx.is_some();
                                    let is_downbeat = (step % spb).is_multiple_of(sp_beat);
                                    let btn = egui::Button::new(if is_on { "■" } else { "·" })
                                        .fill(if is_on {
                                            egui::Color32::from_rgb(70, 175, 235)
                                        } else if is_downbeat {
                                            egui::Color32::from_rgb(42, 46, 58)
                                        } else {
                                            egui::Color32::from_rgb(28, 31, 40)
                                        });
                                    if ui.add_sized([26.0, 26.0], btn).clicked() {
                                        if let Some(idx) = existing_idx {
                                            sec_pat.accompaniment_notes.remove(idx);
                                        } else {
                                            sec_pat.accompaniment_notes.push(midi::MidiNote {
                                                start_tick: s_tick,
                                                duration_ticks: step_ticks * 2 - 10,
                                                channel: target_ch,
                                                note: pitch,
                                                velocity: 92,
                                            });
                                        }
                                        mark_dirty = true;
                                    }
                                    if (step % spb) % sp_beat == sp_beat - 1 || step % spb == spb - 1
                                    {
                                        ui.add_space(5.0);
                                    }
                                }
                            });
                            ui.add_space(3.0);
                        }
                    });

                ui.add_space(8.0);
                ui.separator();
                ui.add_space(6.0);
                ui.horizontal(|ui| {
                    if ui
                        .button(format!(
                            "🧹 Clear {} Notes in Section",
                            self.selected_accomp_part.short_label()
                        ))
                        .clicked()
                    {
                        sec_pat
                            .accompaniment_notes
                            .retain(|n| n.channel != target_ch);
                        mark_dirty = true;
                    }
                });
            });

            // Right Column: Yamaha MIDI Song to Style Slicer & Channel Router
            studio_card_frame().show(&mut cols[1], |ui| {
                ui.heading("✂ MIDI Song-to-Style Bar Slicer (DAW / .MID Importer)");
                ui.add_space(4.0);
                ui.label(
                    egui::RichText::new(
                        "Inspired by Yamaha's MIDI Song to Style app: load any multi-track MIDI song, pick a bar range, route channels 1–16 to PSR-E583 accompaniment parts, and slice directly into any style section.",
                    )
                    .small()
                    .color(egui::Color32::GRAY),
                );
                ui.add_space(6.0);
                ui.separator();
                ui.add_space(6.0);

                ui.horizontal(|ui| {
                    if ui.button("📂 Load .MID / .STY File into Slicer...").clicked() {
                        self.import_sty_or_mid_dialog();
                    }
                    ui.label(egui::RichText::new(&self.slicer.status_message).small());
                });

                ui.add_space(6.0);

                ui.horizontal_wrapped(|ui| {
                    ui.label("Target Section:");
                    egui::ComboBox::from_id_salt("slicer_target_sec")
                        .selected_text(self.slicer.target_section.as_marker_str())
                        .show_ui(ui, |ui| {
                            for &sec in StyleSection::ALL {
                                ui.selectable_value(
                                    &mut self.slicer.target_section,
                                    sec,
                                    sec.as_marker_str(),
                                );
                            }
                        });

                    ui.label("Start Bar:");
                    ui.add(egui::DragValue::new(&mut self.slicer.start_bar).range(1..=512));
                    ui.label("Bars:");
                    ui.add(egui::DragValue::new(&mut self.slicer.bar_count).range(1..=8));
                });

                ui.add_space(4.0);
                ui.checkbox(
                    &mut self.slicer.quantize_drums_to_grid,
                    "Quantize imported Rhythm (Ch 9/10) notes into Drum Machine 16-step grid",
                );

                ui.add_space(6.0);
                ui.separator();
                ui.add_space(4.0);
                ui.label(
                    egui::RichText::new("Source MIDI Channel Routing (Ch 1–16 -> PSR-E583 Part):")
                        .strong(),
                );
                ui.add_space(4.0);

                egui::ScrollArea::vertical()
                    .id_salt("slicer_routing_scroll")
                    .max_height(240.0)
                    .show(ui, |ui| {
                        egui::Grid::new("slicer_ch_grid")
                            .striped(true)
                            .spacing([12.0, 8.0])
                            .num_columns(4)
                            .show(ui, |ui| {
                                for ch in 0..16 {
                                    let note_count = self
                                        .slicer
                                        .parsed
                                        .as_ref()
                                        .map(|p| {
                                            p.notes
                                                .iter()
                                                .filter(|n| n.channel as usize == ch)
                                                .count()
                                        })
                                        .unwrap_or(0);
                                    ui.label(format!("Src Ch {:02} ({} notes):", ch + 1, note_count));
                                    let cur = self.slicer.channel_routing[ch];
                                    egui::ComboBox::from_id_salt(format!("route_ch_{}", ch))
                                        .selected_text(
                                            cur.map(|p| p.short_label()).unwrap_or("Ignore / Mute"),
                                        )
                                        .width(130.0)
                                        .show_ui(ui, |ui| {
                                            ui.selectable_value(
                                                &mut self.slicer.channel_routing[ch],
                                                None,
                                                "Ignore / Mute",
                                            );
                                            for &p in AccompanimentPart::ALL {
                                                ui.selectable_value(
                                                    &mut self.slicer.channel_routing[ch],
                                                    Some(p),
                                                    p.short_label(),
                                                );
                                            }
                                        });
                                    if ch % 2 == 1 {
                                        ui.end_row();
                                    }
                                }
                            });
                    });

                ui.add_space(10.0);
                if ui
                    .button(
                        egui::RichText::new(format!(
                            "⚡ Slice Bars {}..{} into {}",
                            self.slicer.start_bar,
                            self.slicer.start_bar + self.slicer.bar_count - 1,
                            self.slicer.target_section.as_marker_str()
                        ))
                        .strong(),
                    )
                    .clicked()
                {
                    let slicer_snapshot = self.slicer.clone();
                    match self.project.slice_from_smf(&slicer_snapshot) {
                        Ok(cnt) => {
                            self.active_section = slicer_snapshot.target_section;
                            mark_dirty = true;
                            self.status_banner = format!(
                                "Sliced {} notes from bars {}..={} into '{}'",
                                cnt,
                                slicer_snapshot.start_bar,
                                slicer_snapshot.start_bar + slicer_snapshot.bar_count - 1,
                                slicer_snapshot.target_section.as_marker_str()
                            );
                        }
                        Err(e) => {
                            self.status_banner = e;
                        }
                    }
                }
            });
        });

        if mark_dirty {
            self.mark_dirty();
        }
    }

    fn render_voice_mixer_tab(&mut self, ui: &mut egui::Ui) {
        let mut mark_dirty = false;

        ui.columns(2, |cols| {
            // Left: 8-Channel Mixer Strips (Measure 1 SInt Setup)
            studio_card_frame().show(&mut cols[0], |ui| {
                ui.heading("🎛 Measure 1 (SInt) 8-Channel Accompaniment Mixer");
                ui.add_space(4.0);
                ui.label(
                    egui::RichText::new(
                        "Writes Bank Select MSB (CC#0), LSB (CC#32), Program Change, Volume (CC#7), Pan (CC#10), Reverb (CC#91), and Chorus (CC#93) into Measure 1.",
                    )
                    .small()
                    .color(egui::Color32::GRAY),
                );
                ui.add_space(6.0);
                ui.separator();
                ui.add_space(6.0);

                egui::Grid::new("mixer_strips_grid")
                    .striped(true)
                    .spacing([12.0, 10.0])
                    .show(ui, |ui| {
                        ui.label(egui::RichText::new("Part").strong());
                        ui.label(egui::RichText::new("Assigned PSR-E583 Voice").strong());
                        ui.label(egui::RichText::new("MSB/LSB/PC").strong());
                        ui.label(egui::RichText::new("Vol / Pan / Rev / Cho").strong());
                        ui.end_row();

                        for cfg in &mut self.project.channel_configs {
                            let is_selected = self.editing_voice_part == cfg.part;
                            if ui
                                .selectable_label(is_selected, cfg.part.short_label())
                                .clicked()
                            {
                                self.editing_voice_part = cfg.part;
                            }

                            let v_name = PSR_E583_VOICES
                                .iter()
                                .find(|v| v.voice_no == cfg.voice_no)
                                .map(|v| format!("#{:03} {}", v.voice_no, v.name))
                                .unwrap_or_else(|| format!("Voice #{}", cfg.voice_no));

                            if ui.button(v_name).clicked() {
                                self.editing_voice_part = cfg.part;
                            }

                            ui.label(
                                egui::RichText::new(format!(
                                    "{:03} / {:03} / {:03}",
                                    cfg.bank_msb,
                                    cfg.bank_lsb,
                                    cfg.program_change + 1
                                ))
                                .monospace(),
                            );

                            ui.horizontal(|ui| {
                                if ui.checkbox(&mut cfg.muted, "Mute").changed() {
                                    mark_dirty = true;
                                }
                                if ui
                                    .add(
                                        egui::DragValue::new(&mut cfg.volume)
                                            .range(0..=127)
                                            .prefix("V:"),
                                    )
                                    .changed()
                                {
                                    mark_dirty = true;
                                }
                                if ui
                                    .add(
                                        egui::DragValue::new(&mut cfg.pan)
                                            .range(0..=127)
                                            .prefix("P:"),
                                    )
                                    .changed()
                                {
                                    mark_dirty = true;
                                }
                                if ui
                                    .add(
                                        egui::DragValue::new(&mut cfg.reverb)
                                            .range(0..=127)
                                            .prefix("R:"),
                                    )
                                    .changed()
                                {
                                    mark_dirty = true;
                                }
                                if ui
                                    .add(
                                        egui::DragValue::new(&mut cfg.chorus)
                                            .range(0..=127)
                                            .prefix("C:"),
                                    )
                                    .changed()
                                {
                                    mark_dirty = true;
                                }
                            });
                            ui.end_row();
                        }
                    });
            });

            // Right: Searchable PSR-E583 890-Voice Browser
            studio_card_frame().show(&mut cols[1], |ui| {
                ui.heading(format!(
                    "🔍 PSR-E583 Voice Browser -> Assigning to {}",
                    self.editing_voice_part.short_label()
                ));
                ui.add_space(6.0);

                ui.horizontal(|ui| {
                    ui.label("Search:");
                    ui.text_edit_singleline(&mut self.voice_search_query);
                    if ui.button("Clear").clicked() {
                        self.voice_search_query.clear();
                    }
                });

                ui.add_space(4.0);
                let categories = [
                    "All",
                    "Piano",
                    "E.Piano",
                    "Organ",
                    "Accordion",
                    "Guitar",
                    "Bass",
                    "Strings",
                    "Brass",
                    "Woodwind",
                    "Synth",
                    "Percussion",
                    "World",
                    "Drum Kits",
                    "XGlite / GM",
                ];
                ui.horizontal_wrapped(|ui| {
                    for cat in categories {
                        ui.selectable_value(&mut self.voice_category_filter, cat.to_string(), cat);
                    }
                });
                ui.add_space(4.0);
                ui.separator();
                ui.add_space(4.0);

                let q = self.voice_search_query.trim().to_ascii_lowercase();
                let mut chosen_voice: Option<u16> = None;

                egui::ScrollArea::vertical()
                    .id_salt("voice_browser_scroll")
                    .auto_shrink([false, false])
                    .show(ui, |ui| {
                        egui::Grid::new("voice_browser_table")
                            .striped(true)
                            .spacing([12.0, 6.0])
                            .num_columns(4)
                            .show(ui, |ui| {
                                ui.label(egui::RichText::new("No.").strong());
                                ui.label(egui::RichText::new("Voice Name").strong());
                                ui.label(egui::RichText::new("Category").strong());
                                ui.label(egui::RichText::new("MSB / LSB / PC#").strong());
                                ui.end_row();

                                for v in PSR_E583_VOICES.iter() {
                                    if !v.is_midi_selectable() {
                                        continue;
                                    }
                                    if self.voice_category_filter != "All"
                                        && v.category != self.voice_category_filter
                                    {
                                        continue;
                                    }
                                    if !q.is_empty()
                                        && !v.name.to_ascii_lowercase().contains(&q)
                                        && !v.voice_no.to_string().contains(&q)
                                    {
                                        continue;
                                    }

                                    ui.label(
                                        egui::RichText::new(format!("#{:03}", v.voice_no))
                                            .monospace(),
                                    );
                                    if ui.selectable_label(false, v.name).clicked() {
                                        chosen_voice = Some(v.voice_no);
                                    }
                                    ui.label(v.category);
                                    ui.label(
                                        egui::RichText::new(format!(
                                            "{:3} / {:3} / {:3}",
                                            v.msb.unwrap_or(0),
                                            v.lsb.unwrap_or(0),
                                            v.pc.unwrap_or(0) + 1
                                        ))
                                        .monospace(),
                                    );
                                    ui.end_row();
                                }
                            });
                    });

                if let Some(v_no) = chosen_voice {
                    self.project.set_channel_voice(self.editing_voice_part, v_no);
                    mark_dirty = true;
                    if let Some(v) = PSR_E583_VOICES.iter().find(|x| x.voice_no == v_no) {
                        self.status_banner = format!(
                            "Assigned Voice #{:03} '{}' (MSB:{}, LSB:{}, PC:{}) to {}",
                            v.voice_no,
                            v.name,
                            v.msb.unwrap_or(0),
                            v.lsb.unwrap_or(0),
                            v.pc.unwrap_or(0) + 1,
                            self.editing_voice_part.short_label()
                        );
                    }
                }
            });
        });

        if mark_dirty {
            self.sync_midi_channel_configs();
            self.mark_dirty();
        }
    }

    fn render_casm_inspector_tab(&mut self, ui: &mut egui::Ui) {
        let mut mark_dirty = false;

        ui.columns(2, |cols| {
            // Left: CASM / Ctab 27-Byte Channel Transposition Rule Editor
            studio_card_frame().show(&mut cols[0], |ui| {
                ui.heading("🧬 CASM -> CSEG -> Ctab (27-Byte Channel Rules)");
                ui.add_space(4.0);
                ui.label(
                    egui::RichText::new(
                        "Controls how the PSR-E583 arranger engine routes channels 9–16 and transposes notes when you play chords on the left hand.",
                    )
                    .small()
                    .color(egui::Color32::GRAY),
                );
                ui.add_space(6.0);
                ui.separator();
                ui.add_space(6.0);

                if let Some(seg) = self.project.casm.segments.first_mut() {
                    if ui
                        .checkbox(
                            &mut seg.include_cntt,
                            "Emit SFF1 'Cntt' Extension Chunks (Recommended for Bass On flag)",
                        )
                        .changed()
                    {
                        mark_dirty = true;
                    }
                    ui.add_space(6.0);
                    ui.separator();
                    ui.add_space(6.0);

                    egui::Grid::new("ctab_editor_grid")
                        .striped(true)
                        .spacing([10.0, 10.0])
                        .show(ui, |ui| {
                            ui.label(egui::RichText::new("Ch").strong());
                            ui.label(egui::RichText::new("Track Name").strong());
                            ui.label(egui::RichText::new("NTR (Root)").strong());
                            ui.label(egui::RichText::new("NTT (Table)").strong());
                            ui.label(egui::RichText::new("RTR (Retrig)").strong());
                            ui.label(egui::RichText::new("Note Limits").strong());
                            ui.end_row();

                            for ctab in &mut seg.ctabs {
                                ui.label(
                                    egui::RichText::new(format!("Ch {}", ctab.dest_channel + 1))
                                        .strong(),
                                );
                                if ui
                                    .add(
                                        egui::TextEdit::singleline(&mut ctab.track_name)
                                            .desired_width(72.0),
                                    )
                                    .changed()
                                {
                                    mark_dirty = true;
                                }

                                egui::ComboBox::from_id_salt(format!("ntr_{}", ctab.dest_channel))
                                    .selected_text(ctab.ntr.label())
                                    .width(125.0)
                                    .show_ui(ui, |ui| {
                                        if ui
                                            .selectable_value(
                                                &mut ctab.ntr,
                                                NtrRule::RootTransposition,
                                                NtrRule::RootTransposition.label(),
                                            )
                                            .changed()
                                        {
                                            mark_dirty = true;
                                        }
                                        if ui
                                            .selectable_value(
                                                &mut ctab.ntr,
                                                NtrRule::RootFixed,
                                                NtrRule::RootFixed.label(),
                                            )
                                            .changed()
                                        {
                                            mark_dirty = true;
                                        }
                                    });

                                egui::ComboBox::from_id_salt(format!("ntt_{}", ctab.dest_channel))
                                    .selected_text(ctab.ntt.label())
                                    .width(130.0)
                                    .show_ui(ui, |ui| {
                                        for &t in NttTable::ALL {
                                            if ui
                                                .selectable_value(&mut ctab.ntt, t, t.label())
                                                .changed()
                                            {
                                                mark_dirty = true;
                                            }
                                        }
                                    });

                                egui::ComboBox::from_id_salt(format!("rtr_{}", ctab.dest_channel))
                                    .selected_text(ctab.rtr.label())
                                    .width(135.0)
                                    .show_ui(ui, |ui| {
                                        for &r in RetriggerRule::ALL {
                                            if ui
                                                .selectable_value(&mut ctab.rtr, r, r.label())
                                                .changed()
                                            {
                                                mark_dirty = true;
                                            }
                                        }
                                    });

                                ui.horizontal(|ui| {
                                    if ui
                                        .add(
                                            egui::DragValue::new(&mut ctab.note_limit_low)
                                                .range(0..=127)
                                                .prefix("Lo:"),
                                        )
                                        .changed()
                                    {
                                        mark_dirty = true;
                                    }
                                    if ui
                                        .add(
                                            egui::DragValue::new(&mut ctab.note_limit_high)
                                                .range(0..=127)
                                                .prefix("Hi:"),
                                        )
                                        .changed()
                                    {
                                        mark_dirty = true;
                                    }
                                });
                                ui.end_row();
                            }
                        });
                }
            });

            // Right: 10-Point PSR-E583 Validation Report & CASM Hex Inspector
            studio_card_frame().show(&mut cols[1], |ui| {
                if mark_dirty {
                    self.mark_dirty();
                }
                self.ensure_compiled();

                ui.heading("🛡 PSR-E583 10-Point Binary Verification");
                ui.add_space(6.0);
                for check in &self.cached_report.checks {
                    ui.horizontal(|ui| {
                        let icon = if check.passed { "✔" } else { "✖" };
                        let color = if check.passed {
                            egui::Color32::from_rgb(90, 225, 135)
                        } else {
                            egui::Color32::from_rgb(255, 100, 100)
                        };
                        ui.label(egui::RichText::new(icon).strong().color(color));
                        ui.label(egui::RichText::new(check.name).strong());
                        ui.label(
                            egui::RichText::new(&check.detail)
                                .small()
                                .color(egui::Color32::GRAY),
                        );
                    });
                }

                ui.add_space(8.0);
                ui.separator();
                ui.add_space(6.0);
                ui.heading("🔬 Live FF 2F 00 + CASM Binary Hex Dump");
                ui.add_space(4.0);

                let seam_start = self.cached_report.smf_bytes.saturating_sub(16);
                let slice = &self.cached_sty_bytes[seam_start..];

                egui::ScrollArea::vertical()
                    .id_salt("hex_inspector_scroll")
                    .auto_shrink([false, false])
                    .show(ui, |ui| {
                        for (row_idx, chunk) in slice.chunks(16).take(48).enumerate() {
                            let addr = seam_start + row_idx * 16;
                            let mut hex_part = String::with_capacity(48);
                            let mut ascii_part = String::with_capacity(16);
                            for &b in chunk {
                                hex_part.push_str(&format!("{:02X} ", b));
                                ascii_part.push(if b.is_ascii_graphic() || b == b' ' {
                                    b as char
                                } else {
                                    '.'
                                });
                            }
                            ui.label(
                                egui::RichText::new(format!(
                                    "{:04X}: {:<48} |{}|",
                                    addr, hex_part, ascii_part
                                ))
                                .monospace()
                                .small(),
                            );
                        }
                    });
            });
        });
    }
}
