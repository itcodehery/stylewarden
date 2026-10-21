# StyleWarden

**StyleWarden** is a cross-platform Yamaha `.sty` (SFF1) Style Creator and Editor written in Rust, designed with first-class compatibility for the **Yamaha PSR-E583** Electronic Arranger Keyboard.

## Overview

StyleWarden streamlines creating custom accompaniment styles (`.sty`) without relying solely on a traditional DAW or manual hex editing:

- **SFF1 `.sty` Engine**: Reads, validates, and compiles Standard MIDI File Format 0 (SMF0) tracks with Yamaha `SFF1` / `SInt` initialization markers, section markers (`Intro A/B`, `Main A/B`, `Fill In AA/BB/AB/BA`, `Ending A/B`), and the proprietary `CASM` (`CSEG`, `Sdec`, `Ctab`) chord-assignment chunks.
- **Built-in Drum Machine Interface**: Step-sequencer grid for programming **Rhy1** (Channel 9) and **Rhy2** (Channel 10) percussion and drum kits directly using onboard PSR-E583 Drum Kit mappings.
- **MIDI Song-to-Style Workflow**: Import multi-track MIDI clips, slice measures into style sections, and route accompaniment tracks (`Bass`, `Chd1`, `Chd2`, `Pad`, `Phr1`, `Phr2` on Channels 9–16).
- **PSR-E583 Voice & Drum Kit Database**: Native Bank Select (`MSB` CC#0, `LSB` CC#32) and Program Change presets mapped directly from the official Yamaha PSR-E483/PSR-E583 Data List.

## Target Format Specification (SFF1)

| Channel | Index | Accompaniment Part | Default Role |
| :--- | :--- | :--- | :--- |
| **Ch 9** | `0x08` | `Rhy1` | Sub-Rhythm / Percussion |
| **Ch 10** | `0x09` | `Rhy2` | Main Drum Kit |
| **Ch 11** | `0x0A` | `Bass` | Bassline (written in C Maj7) |
| **Ch 12** | `0x0B` | `Chd1` | Chord 1 (written in C Maj7) |
| **Ch 13** | `0x0C` | `Chd2` | Chord 2 (written in C Maj7) |
| **Ch 14** | `0x0D` | `Pad` | Pad (written in C Maj7) |
| **Ch 15** | `0x0E` | `Phr1` | Phrase 1 (written in C Maj7) |
| **Ch 16** | `0x0F` | `Phr2` | Phrase 2 (written in C Maj7) |

## Tech Stack

- **Language**: Rust (`eframe` / `egui` cross-platform native desktop GUI, `midir` USB-MIDI Out, `rfd` native file dialogs)
- **Target Hardware**: Yamaha PSR-E583 (SFF1 / 2-Main Variation architecture)

## Quick Start

```bash
# Launch the native StyleWarden Drum Machine & Style Studio GUI
cargo run --release

# Export all 5 built-in PSR-E583 genre presets (.sty) to ./exports/
cargo run -- --export-presets ./exports

# Run SFF1 & CASM binary verification tests
cargo test
```
