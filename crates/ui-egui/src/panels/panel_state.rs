//! Frontend state of the Lumetri Scopes, Timecode, Events, Progress and Reference Monitor panels
//! (`UiState::panels`). Plain serde, so agents read it with `ui.inspect` (`ui.panels`) and change
//! it with `ui.set {"panels": {"scopes": {...}, "timecode": {...}, ...}}` (fields merged).

use filmcraft_scopes::{Brightness, ColorSpace, ParadeType, Scale, ScopeKind, Targets, WaveformType};
use filmcraft_time::TimeDisplay;
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct PanelsState {
    pub scopes: ScopesState,
    pub timecode: TimecodeState,
    pub reference: ReferenceState,
    pub events: EventsState,
    pub progress: ProgressState,
    pub reverse_engineering: ReverseEngineeringState,
}

/// Lumetri Scopes settings (the wrench / right-click menu and the footer).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct ScopesState {
    /// The scopes shown, in layout order.
    pub shown: Vec<ScopeKind>,
    pub waveform_type: WaveformType,
    pub parade_type: ParadeType,
    pub color_space: ColorSpace,
    pub brightness: Brightness,
    /// The bit-depth dropdown: 8 Bit, Float or HDR.
    pub scale: Scale,
    /// Clamp Signal.
    pub clamp: bool,
    /// YUV vectorscope targets (75 % / 100 %).
    pub targets: Targets,
    /// The preset last applied ("" = custom).
    pub preset: String,
}

impl Default for ScopesState {
    fn default() -> Self {
        ScopesState {
            shown: vec![ScopeKind::Parade],
            waveform_type: WaveformType::Rgb,
            parade_type: ParadeType::Rgb,
            color_space: ColorSpace::Auto,
            brightness: Brightness::Normal,
            scale: Scale::Bits8,
            clamp: true,
            targets: Targets::Percent75,
            preset: String::new(),
        }
    }
}

/// A scopes layout preset: (name, scopes, waveform type, parade type, scale, clamp).
pub type ScopePreset = (&'static str, &'static [ScopeKind], WaveformType, ParadeType, Scale, bool);

/// The Presets submenu (our own names).
pub const SCOPE_PRESETS: [ScopePreset; 8] = {
    use ScopeKind::*;
    [
        ("Waveform (RGB)", &[Waveform], WaveformType::Rgb, ParadeType::Rgb, Scale::Bits8, true),
        ("Waveform (Luma)", &[Waveform], WaveformType::Luma, ParadeType::Rgb, Scale::Bits8, true),
        ("Parade (RGB)", &[Parade], WaveformType::Rgb, ParadeType::Rgb, Scale::Bits8, true),
        ("Vectorscope (YUV)", &[VectorscopeYuv], WaveformType::Rgb, ParadeType::Rgb, Scale::Bits8, true),
        ("Vectorscope + Waveform YC", &[VectorscopeYuv, Waveform], WaveformType::Yc, ParadeType::Rgb, Scale::Bits8, true),
        ("Four Scopes (YUV, float, no clamp)", &[VectorscopeYuv, Histogram, Parade, Waveform], WaveformType::Yc, ParadeType::Yuv, Scale::Float, false),
        ("Four Scopes (RGB, 8 bit)", &[VectorscopeYuv, Histogram, Parade, Waveform], WaveformType::Rgb, ParadeType::Rgb, Scale::Bits8, true),
        ("HLS Vectorscope + Histogram", &[VectorscopeHls, Histogram], WaveformType::Rgb, ParadeType::Rgb, Scale::Bits8, true),
    ]
};

impl ScopesState {
    pub fn apply_preset(&mut self, name: &str) -> bool {
        let Some(p) = SCOPE_PRESETS.iter().find(|p| p.0.eq_ignore_ascii_case(name)) else { return false };
        self.shown = p.1.to_vec();
        self.waveform_type = p.2;
        self.parade_type = p.3;
        self.scale = p.4;
        self.clamp = p.5;
        self.preset = p.0.to_string();
        true
    }
    /// Show or hide a scope (the last one cannot be hidden).
    pub fn toggle(&mut self, k: ScopeKind) {
        if let Some(i) = self.shown.iter().position(|x| *x == k) {
            if self.shown.len() > 1 {
                self.shown.remove(i);
            }
        } else {
            // keep the menu's order
            self.shown.push(k);
            self.shown.sort_by_key(|x| ScopeKind::ALL.iter().position(|a| a == x));
        }
        self.preset.clear();
    }
}

/// What a Timecode panel row measures.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum TcMode {
    /// The playhead (sequence time / source clip time).
    #[default]
    Current,
    /// The media timecode of the clip under the playhead (topmost video clip; the file's own
    /// timecode for the Source monitor).
    Media,
    /// The sequence's / clip's duration.
    Duration,
    /// In to Out duration (the whole duration when no marks).
    InOut,
    /// From the playhead to the end (or to the Out point).
    Remaining,
}

impl TcMode {
    pub const ALL: [TcMode; 5] = [TcMode::Current, TcMode::Media, TcMode::Duration, TcMode::InOut, TcMode::Remaining];
    pub fn label(self) -> &'static str {
        match self {
            TcMode::Current => "Current Time",
            TcMode::Media => "Media Time",
            TcMode::Duration => "Duration",
            TcMode::InOut => "In/Out Duration",
            TcMode::Remaining => "Remaining",
        }
    }
}

/// Which monitor a Timecode row follows.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum TcSource {
    /// The Source monitor when it has focus, else the Program / Timeline.
    #[default]
    Active,
    Program,
    Source,
}

impl TcSource {
    pub const ALL: [TcSource; 3] = [TcSource::Active, TcSource::Program, TcSource::Source];
    pub fn label(self) -> &'static str {
        match self {
            TcSource::Active => "Active Monitor",
            TcSource::Program => "Program",
            TcSource::Source => "Source",
        }
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct TimecodeRow {
    pub source: TcSource,
    pub mode: TcMode,
    pub display: TimeDisplay,
}

/// The Timecode panel: one large row plus optional smaller rows.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct TimecodeState {
    pub rows: Vec<TimecodeRow>,
    /// Show the clip / sequence name under the timecode.
    pub show_name: bool,
}

impl Default for TimecodeState {
    fn default() -> Self {
        TimecodeState { rows: vec![TimecodeRow::default()], show_name: true }
    }
}

/// What the Reference Monitor shows.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum RefDisplay {
    #[default]
    Composite,
    /// Lumetri Scopes of the reference frame (the panel's scope selection).
    Scopes,
}

/// The Reference Monitor: a second view of the active sequence, ganged to the Program monitor or
/// parked on its own frame.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct ReferenceState {
    /// Gang to Program Monitor: follow the playhead.
    pub ganged: bool,
    /// The reference time (ticks) when not ganged.
    pub time: i64,
    pub display: RefDisplay,
    /// Scopes shown in Scopes mode.
    pub scopes: Vec<ScopeKind>,
}

impl Default for ReferenceState {
    fn default() -> Self {
        ReferenceState { ganged: false, time: 0, display: RefDisplay::Composite, scopes: vec![ScopeKind::VectorscopeYuv, ScopeKind::Waveform] }
    }
}

/// The Events panel.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct EventsState {
    /// Lowest level shown ("info", "warning", "error").
    pub level: String,
    /// The selected entry (details shown below the list).
    pub selected: Option<u64>,
    /// Entries seen when the panel was last shown (newer ones are "new").
    pub seen: u64,
}

/// The Progress panel.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct ProgressState {
    /// Also list finished jobs.
    pub show_finished: bool,
}

impl Default for ProgressState {
    fn default() -> Self {
        ProgressState { show_finished: true }
    }
}

/// A frozen source-frame request, separate from the editable project.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct ReverseEngineeringState {
    pub capture: Option<ReverseEngineeringCapture>,
    pub range_mode: bool,
    pub range_clip: Option<filmcraft_project::ClipId>,
    pub range_sequence: Option<filmcraft_project::ItemId>,
    pub start_frame: i64,
    pub end_frame: i64,
    #[serde(default = "default_sample_interval")]
    pub sample_interval: i64,
    pub samples: Vec<ReverseEngineeringCapture>,
    pub motion: Option<super::reverse_motion::MotionReport>,
    pub active_tool: String,
    pub global_motion: Option<super::reverse_tools::GlobalReport>,
    pub local_motion: Option<super::reverse_local::LocalReport>,
    pub local_show_raw: bool,
    pub motion_regions: Option<super::reverse_regions::RegionReport>,
    pub discontinuities: Option<super::reverse_discontinuities::Report>,
    pub motion_time: Option<super::reverse_time::TimeReport>,
    pub sam_report: Option<super::reverse_sam::Report>,
    pub sam_captures: Vec<ReverseEngineeringCapture>,
    pub sam_objects: Vec<filmcraft_vision::sam2::Object>,
    pub sam_name: String,
    pub sam_frame: usize,
    pub sam_foreground: bool,
    pub sam_binary_view: bool,
    pub sam_colors: std::collections::BTreeMap<String, [u8; 3]>,
    pub sam_device: filmcraft_vision::execution::Device,
    pub tracking_report: Option<super::reverse_tracking::TrackingReport>,
    pub tracking_labels: Vec<String>,
    pub tracking_group: String,
    pub tracking_queries: Vec<filmcraft_vision::point_tracking::Query>,
    pub tracking_captures: Vec<ReverseEngineeringCapture>,
    pub tracking_grid: bool,
    pub tracking_grid_side: usize,
    pub tracking_frame: usize,
    pub flow_device: filmcraft_vision::execution::Device,
    pub tracking_device: filmcraft_vision::execution::Device,
    pub saved_results: Vec<super::reverse_tools::SavedResult>,
    pub global_tolerance: f64,
    pub global_show_support: bool,
}

fn default_sample_interval() -> i64 {
    30
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ReverseEngineeringCapture {
    pub item: filmcraft_project::ItemId,
    pub media_time: filmcraft_time::Tick,
    pub timeline_time: filmcraft_time::Tick,
    pub name: String,
}

impl Default for ReverseEngineeringState {
    fn default() -> Self {
        Self {
            capture: None,
            range_mode: true,
            range_clip: None,
            range_sequence: None,
            start_frame: 0,
            end_frame: 0,
            sample_interval: 30,
            samples: Vec::new(),
            motion: None,
            active_tool: "menu".into(),
            global_motion: None,
            local_motion: None,
            local_show_raw: false,
            motion_regions: None,
            discontinuities: None,
            motion_time: None,
            sam_report: None,
            sam_captures: Vec::new(),
            sam_objects: Vec::new(),
            sam_name: "Sun".into(),
            sam_frame: 0,
            sam_foreground: true,
            sam_binary_view: false,
            sam_colors: Default::default(),
            sam_device: Default::default(),
            tracking_report: None,
            tracking_labels: Vec::new(),
            tracking_group: "Unlabelled".into(),
            tracking_queries: Vec::new(),
            tracking_captures: Vec::new(),
            tracking_grid: false,
            tracking_grid_side: 6,
            tracking_frame: 0,
            flow_device: Default::default(),
            tracking_device: Default::default(),
            saved_results: Vec::new(),
            global_tolerance: 0.5,
            global_show_support: false,
        }
    }
}
