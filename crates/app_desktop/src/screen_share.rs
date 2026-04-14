use std::collections::VecDeque;
use std::io::Cursor;
use std::rc::Rc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use tokio::sync::{Mutex as TokioMutex, Notify};
use ui_shell::{MainWindow, ShareSourceData};
use xcap::image::codecs::jpeg::JpegEncoder;
use xcap::image::imageops::FilterType;
use xcap::{Monitor, Window};

const SCREEN_PRESSURE_THRESHOLD: u8 = 3;
const SCREEN_SHARE_PREVIEW_MAX_WIDTH: u32 = 420;
const SCREEN_SHARE_PREVIEW_INTERVAL: Duration = Duration::from_millis(90);
const SCREEN_TRANSPORT_FEEDBACK_WINDOW: Duration = Duration::from_secs(4);
const SCREEN_TRANSPORT_DROP_RATE_THRESHOLD: f32 = 0.12;
const SCREEN_TRANSPORT_PACING_RECOVERY_WINDOWS: u8 = 2;

const SHARE_PROFILES: [ShareProfile; 4] = [
    ShareProfile {
        name: "Efficient",
        detail: "Default low-cost mode. Tops out at 540p / 8 fps and keeps CPU use conservative.",
        presets: [
            QualityPreset {
                name: "540p / 8 fps",
                max_width: 960,
                fps: 8,
                jpeg_quality: 55,
                detail: "Best fit for smooth voice-first calls with minimal system load.",
            },
            QualityPreset {
                name: "480p / 6 fps",
                max_width: 854,
                fps: 6,
                jpeg_quality: 48,
                detail: "Drops resolution and motion to recover quickly under pressure.",
            },
            QualityPreset {
                name: "360p / 5 fps",
                max_width: 640,
                fps: 5,
                jpeg_quality: 40,
                detail: "Lowest-impact fallback when capture or network pressure stays high.",
            },
        ],
    },
    ShareProfile {
        name: "Balanced",
        detail: "Readable desktop capture at up to 720p / 15 fps without pushing too hard.",
        presets: [
            QualityPreset {
                name: "720p / 15 fps",
                max_width: 1280,
                fps: 15,
                jpeg_quality: 52,
                detail: "Good general-purpose profile for docs, UI walkthroughs, and light motion.",
            },
            QualityPreset {
                name: "540p / 12 fps",
                max_width: 960,
                fps: 12,
                jpeg_quality: 46,
                detail: "Keeps text readable while reducing encode and bandwidth pressure.",
            },
            QualityPreset {
                name: "480p / 8 fps",
                max_width: 854,
                fps: 8,
                jpeg_quality: 40,
                detail: "Safe recovery step when the sender or viewer starts falling behind.",
            },
        ],
    },
    ShareProfile {
        name: "Smooth",
        detail: "Higher motion profile with a 720p / 30 fps ceiling. Costs more while live.",
        presets: [
            QualityPreset {
                name: "720p / 30 fps",
                max_width: 1280,
                fps: 30,
                jpeg_quality: 46,
                detail:
                    "Better for UI animation and scrolling, but noticeably heavier than Balanced.",
            },
            QualityPreset {
                name: "720p / 20 fps",
                max_width: 1280,
                fps: 20,
                jpeg_quality: 40,
                detail: "Keeps the same 720p frame while trimming motion and encode cost.",
            },
            QualityPreset {
                name: "540p / 12 fps",
                max_width: 960,
                fps: 12,
                jpeg_quality: 36,
                detail: "Fallback once the sender or viewer cannot keep pace with smooth mode.",
            },
        ],
    },
    ShareProfile {
        name: "Max",
        detail: "Attempts up to 720p / 60 fps. This is expensive and should stay optional.",
        presets: [
            QualityPreset {
                name: "720p / 60 fps",
                max_width: 1280,
                fps: 60,
                jpeg_quality: 40,
                detail:
                    "Fastest option. Expect much higher CPU, bandwidth, and decode load while live.",
            },
            QualityPreset {
                name: "720p / 30 fps",
                max_width: 1280,
                fps: 30,
                jpeg_quality: 36,
                detail: "Automatic recovery step when a full 60 fps stream cannot be sustained.",
            },
            QualityPreset {
                name: "540p / 15 fps",
                max_width: 960,
                fps: 15,
                jpeg_quality: 34,
                detail: "Lowest safe fallback if Max mode remains under pressure.",
            },
        ],
    },
];

#[derive(Clone, Copy, Debug)]
struct ShareProfile {
    name: &'static str,
    detail: &'static str,
    presets: [QualityPreset; 3],
}

#[derive(Clone, Copy, Debug)]
struct QualityPreset {
    name: &'static str,
    max_width: u32,
    fps: u64,
    jpeg_quality: u8,
    detail: &'static str,
}

#[derive(Clone, Debug, Default)]
pub struct ScreenShareSourceDescriptor {
    pub id: String,
    pub label: String,
    pub detail: String,
    pub kind_label: String,
}

#[derive(Clone, Debug, Default)]
pub struct ScreenShareUiState {
    pub sources: Vec<ScreenShareSourceDescriptor>,
    pub selected_index: i32,
    pub selected_profile: i32,
    pub source_label: String,
    pub source_detail: String,
    pub quality_label: String,
    pub quality_detail: String,
}

#[derive(Clone, Debug)]
enum CaptureTarget {
    Display(Monitor),
    Window(Window),
}

// SAFETY: Monitor/Window contain OS handles (HMONITOR, HWND) that are valid process-wide.
// We only use them for read-only frame capture, which is safe across threads.
unsafe impl Send for CaptureTarget {}

#[derive(Clone, Debug)]
struct CaptureSource {
    id: String,
    label: String,
    detail: String,
    kind_label: String,
    target: CaptureTarget,
}

#[derive(Clone, Debug)]
struct ShareUiStatus {
    source_label: String,
    source_detail: String,
    quality_label: String,
    quality_detail: String,
}

#[derive(Clone, Debug, Default)]
struct PreviewFrame {
    width: u32,
    height: u32,
    pixels: Vec<u8>,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
struct TransportPacer {
    level: u8,
    stable_windows: u8,
}

impl TransportPacer {
    fn level(self) -> u8 {
        self.level
    }

    fn paced_interval(self, base: Duration) -> Duration {
        base.mul_f32(match self.level {
            0 => 1.0,
            1 => 1.35,
            _ => 1.75,
        })
    }

    fn record_feedback(&mut self, summary: TransportFeedbackSummary) -> bool {
        if summary.version == 0 {
            return false;
        }

        if summary.is_under_pressure() {
            self.stable_windows = 0;
            let next = self.level.saturating_add(1).min(2);
            let changed = next != self.level;
            self.level = next;
            return changed;
        }

        self.stable_windows = self.stable_windows.saturating_add(1);
        if self.stable_windows >= SCREEN_TRANSPORT_PACING_RECOVERY_WINDOWS && self.level > 0 {
            self.level -= 1;
            self.stable_windows = 0;
            return true;
        }
        false
    }
}

#[derive(Clone, Copy, Debug, Default)]
struct TransportFeedbackSample {
    frames_completed: u32,
    frames_dropped: u32,
    frames_timed_out: u32,
}

#[derive(Clone, Copy, Debug, Default)]
struct TransportFeedbackSummary {
    version: u64,
    frames_completed: u32,
    frames_dropped: u32,
    frames_timed_out: u32,
}

impl TransportFeedbackSummary {
    fn is_under_pressure(self) -> bool {
        let total = self
            .frames_completed
            .saturating_add(self.frames_dropped)
            .saturating_add(self.frames_timed_out);
        if total == 0 {
            return false;
        }
        let troubled = self.frames_dropped.saturating_add(self.frames_timed_out);
        self.frames_timed_out > 0
            || (troubled as f32 / total as f32) >= SCREEN_TRANSPORT_DROP_RATE_THRESHOLD
    }
}

#[derive(Clone, Copy, Debug)]
struct TimedTransportFeedback {
    sample: TransportFeedbackSample,
    recorded_at: Instant,
}

#[derive(Debug, Default)]
struct TransportFeedbackState {
    version: u64,
    reports: VecDeque<TimedTransportFeedback>,
}

impl TransportFeedbackState {
    fn record(&mut self, sample: TransportFeedbackSample) {
        let now = Instant::now();
        self.prune(now);
        self.reports.push_back(TimedTransportFeedback {
            sample,
            recorded_at: now,
        });
        self.version = self.version.wrapping_add(1);
    }

    fn summary(&mut self) -> TransportFeedbackSummary {
        let now = Instant::now();
        self.prune(now);
        let mut summary = TransportFeedbackSummary {
            version: self.version,
            ..TransportFeedbackSummary::default()
        };
        for report in &self.reports {
            summary.frames_completed = summary
                .frames_completed
                .saturating_add(report.sample.frames_completed);
            summary.frames_dropped = summary
                .frames_dropped
                .saturating_add(report.sample.frames_dropped);
            summary.frames_timed_out = summary
                .frames_timed_out
                .saturating_add(report.sample.frames_timed_out);
        }
        summary
    }

    fn clear(&mut self) {
        self.reports.clear();
        self.version = self.version.wrapping_add(1);
    }

    fn prune(&mut self, now: Instant) {
        while self.reports.front().is_some_and(|report| {
            now.duration_since(report.recorded_at) > SCREEN_TRANSPORT_FEEDBACK_WINDOW
        }) {
            self.reports.pop_front();
        }
    }
}

#[derive(Clone, Debug)]
struct CaptureControl {
    stop: Arc<AtomicBool>,
    frame_ready: Arc<Notify>,
}

impl Default for ShareUiStatus {
    fn default() -> Self {
        let profile = SHARE_PROFILES[0];
        Self {
            source_label: "Primary display".into(),
            source_detail: "Ready to share a display or window when the room allows it.".into(),
            quality_label: profile.name.into(),
            quality_detail: profile.detail.into(),
        }
    }
}

#[derive(Default)]
pub struct ScreenShareController {
    stop_flag: Mutex<Option<CaptureControl>>,
    selected_source_id: Mutex<Option<String>>,
    selected_profile_index: Mutex<usize>,
    cached_sources: Mutex<Vec<ScreenShareSourceDescriptor>>,
    ui_status: Mutex<ShareUiStatus>,
    preview_frame: Arc<Mutex<Option<PreviewFrame>>>,
    transport_feedback: Arc<Mutex<TransportFeedbackState>>,
}

impl ScreenShareController {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn refresh_sources(&self) -> Result<(), String> {
        let sources = enumerate_sources()?;
        let previous_id = self
            .selected_source_id
            .lock()
            .map_err(|_| "Screen share selection is unavailable".to_string())?
            .clone();
        let selected_index = default_source_index(&sources, previous_id.as_deref());
        let selected_id = selected_index
            .and_then(|index| sources.get(index))
            .map(|source| source.id.clone());

        {
            let mut cache = self
                .cached_sources
                .lock()
                .map_err(|_| "Screen share source cache is unavailable".to_string())?;
            *cache = sources
                .iter()
                .map(|source| ScreenShareSourceDescriptor {
                    id: source.id.clone(),
                    label: source.label.clone(),
                    detail: source.detail.clone(),
                    kind_label: source.kind_label.clone(),
                })
                .collect();
        }
        {
            let mut current = self
                .selected_source_id
                .lock()
                .map_err(|_| "Screen share selection is unavailable".to_string())?;
            *current = selected_id;
        }
        if !self.capture_active() {
            self.refresh_idle_status();
        }
        Ok(())
    }

    pub fn select_source_index(&self, index: usize) -> Result<(), String> {
        let selected = self
            .cached_sources
            .lock()
            .map_err(|_| "Screen share source cache is unavailable".to_string())?
            .get(index)
            .cloned()
            .ok_or_else(|| "Screen share source is no longer available".to_string())?;
        {
            let mut current = self
                .selected_source_id
                .lock()
                .map_err(|_| "Screen share selection is unavailable".to_string())?;
            *current = Some(selected.id);
        }
        if !self.capture_active() {
            self.refresh_idle_status();
        }
        Ok(())
    }

    pub fn select_profile_index(&self, index: usize) -> Result<(), String> {
        if index >= SHARE_PROFILES.len() {
            return Err("Screen share profile is not available".to_string());
        }
        {
            let mut selected = self
                .selected_profile_index
                .lock()
                .map_err(|_| "Screen share profile state is unavailable".to_string())?;
            *selected = index;
        }
        if !self.capture_active() {
            self.refresh_idle_status();
        }
        Ok(())
    }

    pub fn apply_to_window(&self, window: &MainWindow) {
        let ui_state = self.ui_state();
        let model: Vec<ShareSourceData> = ui_state
            .sources
            .iter()
            .map(|source| ShareSourceData {
                label: source.label.clone().into(),
                detail: source.detail.clone().into(),
                kind_label: source.kind_label.clone().into(),
            })
            .collect();
        window.set_screen_share_sources(Rc::new(slint::VecModel::from(model)).into());
        window.set_selected_screen_share_source(ui_state.selected_index);
        window.set_selected_screen_share_profile(ui_state.selected_profile);
        window.set_screen_share_source_label(ui_state.source_label.into());
        window.set_screen_share_source_detail(ui_state.source_detail.into());
        window.set_screen_share_quality_label(ui_state.quality_label.into());
        window.set_screen_share_quality_detail(ui_state.quality_detail.into());
    }

    pub fn apply_latest_preview(&self, window: &MainWindow) {
        let preview = self
            .preview_frame
            .lock()
            .ok()
            .and_then(|mut frame| frame.take());
        let Some(preview) = preview else {
            return;
        };
        if preview.width == 0 || preview.height == 0 || preview.pixels.is_empty() {
            return;
        }

        let buffer = slint::SharedPixelBuffer::<slint::Rgba8Pixel>::clone_from_slice(
            &preview.pixels,
            preview.width,
            preview.height,
        );
        window.set_screen_share_image(slint::Image::from_rgba8(buffer));
    }

    pub fn record_transport_feedback(
        &self,
        frames_completed: u32,
        frames_dropped: u32,
        frames_timed_out: u32,
    ) {
        if let Ok(mut feedback) = self.transport_feedback.lock() {
            feedback.record(TransportFeedbackSample {
                frames_completed,
                frames_dropped,
                frames_timed_out,
            });
        }
    }

    pub fn ui_state(&self) -> ScreenShareUiState {
        let sources = self
            .cached_sources
            .lock()
            .map(|cache| cache.clone())
            .unwrap_or_default();
        let selected_id = self
            .selected_source_id
            .lock()
            .ok()
            .and_then(|selected| selected.clone());
        let selected_index = selected_id
            .as_deref()
            .and_then(|id| sources.iter().position(|source| source.id == id))
            .map(|index| index as i32)
            .unwrap_or(-1);
        let selected_profile = self
            .selected_profile_index
            .lock()
            .map(|index| *index as i32)
            .unwrap_or(0);
        let status = self
            .ui_status
            .lock()
            .map(|status| status.clone())
            .unwrap_or_default();
        ScreenShareUiState {
            sources,
            selected_index,
            selected_profile,
            source_label: status.source_label,
            source_detail: status.source_detail,
            quality_label: status.quality_label,
            quality_detail: status.quality_detail,
        }
    }

    pub fn start_capture(
        &self,
        network: Arc<TokioMutex<net_control::NetworkClient>>,
        rt_handle: tokio::runtime::Handle,
    ) -> Result<(), String> {
        let mut state = self
            .stop_flag
            .lock()
            .map_err(|_| "Screen share state is unavailable".to_string())?;
        if state.is_some() {
            return Ok(());
        }

        let sources = enumerate_sources()?;
        {
            let mut cache = self
                .cached_sources
                .lock()
                .map_err(|_| "Screen share source cache is unavailable".to_string())?;
            *cache = sources
                .iter()
                .map(|source| ScreenShareSourceDescriptor {
                    id: source.id.clone(),
                    label: source.label.clone(),
                    detail: source.detail.clone(),
                    kind_label: source.kind_label.clone(),
                })
                .collect();
        }
        let selected_source_id = self
            .selected_source_id
            .lock()
            .map_err(|_| "Screen share selection is unavailable".to_string())?
            .clone();
        let source_index = default_source_index(&sources, selected_source_id.as_deref())
            .ok_or_else(|| "No display or window is available to share".to_string())?;
        let source = sources
            .get(source_index)
            .cloned()
            .ok_or_else(|| "Screen share source is no longer available".to_string())?;
        {
            let mut current = self
                .selected_source_id
                .lock()
                .map_err(|_| "Screen share selection is unavailable".to_string())?;
            *current = Some(source.id.clone());
        }
        let profile = self.selected_profile();
        probe_source(&source.target)?;
        if let Ok(mut preview) = self.preview_frame.lock() {
            *preview = None;
        }

        let control = CaptureControl {
            stop: Arc::new(AtomicBool::new(false)),
            frame_ready: Arc::new(Notify::new()),
        };
        let stop_for_thread = control.stop.clone();
        let stop_for_sender = control.stop.clone();
        let frame_ready = control.frame_ready.clone();
        let frame_ready_for_thread = control.frame_ready.clone();
        let pending_frame = Arc::new(Mutex::new(None::<Vec<u8>>));
        let pending_frame_sender = pending_frame.clone();
        let preview_frame = self.preview_frame.clone();
        let transport_feedback = self.transport_feedback.clone();
        let status = Arc::new(Mutex::new(ShareUiStatus {
            source_label: source.label.clone(),
            source_detail: format!("{} live in this room.", source.detail),
            quality_label: format!("{} · {}", profile.name, profile.presets[0].name),
            quality_detail: profile.presets[0].detail.into(),
        }));
        {
            let mut current_status = self
                .ui_status
                .lock()
                .map_err(|_| "Screen share status is unavailable".to_string())?;
            *current_status = status
                .lock()
                .map_err(|_| "Screen share status is unavailable".to_string())?
                .clone();
        }

        let status_for_thread = status.clone();
        if let Ok(mut feedback) = transport_feedback.lock() {
            feedback.clear();
        }
        rt_handle.spawn(async move {
            loop {
                frame_ready.notified().await;
                let frame = pending_frame_sender
                    .lock()
                    .ok()
                    .and_then(|mut slot| slot.take());
                let Some(frame) = frame else {
                    if stop_for_sender.load(Ordering::Relaxed) {
                        break;
                    }
                    continue;
                };
                let net = network.lock().await;
                if let Err(e) = net.send_screen_frame(&frame).await {
                    log::warn!("Failed to send screen frame: {e}");
                }
            }
        });

        std::thread::spawn(move || {
            let mut adaptive = AdaptiveQuality::new(profile);
            let mut pacer = TransportPacer::default();
            let mut last_preview_at = Instant::now() - SCREEN_SHARE_PREVIEW_INTERVAL;
            let mut last_transport_version = 0u64;
            let mut transport_warning_active = false;
            while !stop_for_thread.load(Ordering::Relaxed) {
                let preset = adaptive.current();
                if let Ok(mut state) = status_for_thread.lock() {
                    state.quality_label = format!("{} · {}", profile.name, preset.name);
                    state.quality_detail =
                        screen_share_detail(preset, transport_warning_active, pacer.level()).into();
                }

                let base_frame_interval = Duration::from_millis(1000 / preset.fps.max(1));
                let frame_start = Instant::now();
                let transport_summary = transport_feedback
                    .lock()
                    .ok()
                    .map(|mut feedback| feedback.summary())
                    .unwrap_or_default();
                let mut transport_pressure = transport_warning_active;
                let mut detail_refresh_needed = false;
                if transport_summary.version != last_transport_version {
                    last_transport_version = transport_summary.version;
                    transport_pressure = transport_summary.is_under_pressure();
                    if pacer.record_feedback(transport_summary) {
                        detail_refresh_needed = true;
                    }
                    if transport_warning_active != transport_pressure {
                        transport_warning_active = transport_pressure;
                        detail_refresh_needed = true;
                    }
                }

                let paced_frame_interval = pacer.paced_interval(base_frame_interval);
                let mut pressure = transport_pressure;
                if detail_refresh_needed {
                    if let Ok(mut state) = status_for_thread.lock() {
                        state.quality_detail =
                            screen_share_detail(preset, transport_warning_active, pacer.level())
                                .into();
                    }
                }

                match capture_frame(&source.target) {
                    Ok(image) => {
                        if last_preview_at.elapsed() >= SCREEN_SHARE_PREVIEW_INTERVAL {
                            let preview =
                                build_preview_frame(&image, SCREEN_SHARE_PREVIEW_MAX_WIDTH);
                            if let Ok(mut slot) = preview_frame.lock() {
                                *slot = Some(preview);
                            }
                            last_preview_at = Instant::now();
                        }
                        match encode_frame(image, preset) {
                            Ok(encoded) => {
                                if encoded.len()
                                    > shared_types::MAX_SCREEN_FRAME_SIZE.saturating_mul(3) / 4
                                {
                                    pressure = true;
                                }
                                if let Ok(mut slot) = pending_frame.lock() {
                                    if slot.replace(encoded).is_some() {
                                        pressure = true;
                                    }
                                    frame_ready_for_thread.notify_one();
                                } else {
                                    pressure = true;
                                }
                            }
                            Err(e) => {
                                pressure = true;
                                log::warn!("Failed to encode screen frame: {e}");
                            }
                        }
                    }
                    Err(e) => {
                        pressure = true;
                        log::warn!("Failed to capture screen frame: {e}");
                    }
                }

                let elapsed = frame_start.elapsed();
                if elapsed >= paced_frame_interval.mul_f32(0.90) {
                    pressure = true;
                }
                let preset_changed = adaptive.record_frame(pressure);
                if preset_changed {
                    let active = adaptive.current();
                    if let Ok(mut state) = status_for_thread.lock() {
                        state.quality_label = format!("{} · {}", profile.name, active.name);
                        state.quality_detail =
                            screen_share_detail(active, transport_warning_active, pacer.level())
                                .into();
                    }
                }

                if elapsed < paced_frame_interval {
                    std::thread::sleep(paced_frame_interval - elapsed);
                }
            }
        });

        *state = Some(control);
        Ok(())
    }

    pub fn stop_capture(&self) {
        if let Ok(mut state) = self.stop_flag.lock() {
            if let Some(control) = state.take() {
                control.stop.store(true, Ordering::Relaxed);
                control.frame_ready.notify_waiters();
            }
        }
        if let Ok(mut feedback) = self.transport_feedback.lock() {
            feedback.clear();
        }
        if let Ok(mut preview) = self.preview_frame.lock() {
            *preview = None;
        }
        self.refresh_idle_status();
    }

    fn selected_profile(&self) -> ShareProfile {
        let index = self
            .selected_profile_index
            .lock()
            .map(|index| (*index).min(SHARE_PROFILES.len().saturating_sub(1)))
            .unwrap_or(0);
        SHARE_PROFILES[index]
    }

    fn capture_active(&self) -> bool {
        self.stop_flag
            .lock()
            .map(|state| state.is_some())
            .unwrap_or(false)
    }

    fn selected_source_descriptor(&self) -> Option<ScreenShareSourceDescriptor> {
        let selected_id = self
            .selected_source_id
            .lock()
            .ok()
            .and_then(|selected| selected.clone());
        let cache = self.cached_sources.lock().ok()?;
        selected_id.and_then(|id| cache.iter().find(|source| source.id == id).cloned())
    }

    fn refresh_idle_status(&self) {
        let selected_source = self.selected_source_descriptor();
        let profile = self.selected_profile();
        if let Ok(mut status) = self.ui_status.lock() {
            if let Some(source) = selected_source {
                status.source_label = source.label;
                status.source_detail = format!("{} ready for your next share.", source.detail);
            } else {
                status.source_label = "Primary display".into();
                status.source_detail =
                    "Ready to share a display or window when the room allows it.".into();
            }
            status.quality_label = profile.name.into();
            status.quality_detail = profile.detail.into();
        }
    }
}

#[derive(Debug)]
struct AdaptiveQuality {
    profile: ShareProfile,
    preset_index: usize,
    pressure_score: u8,
    stable_frames: u16,
}

impl AdaptiveQuality {
    fn new(profile: ShareProfile) -> Self {
        Self {
            profile,
            preset_index: 0,
            pressure_score: 0,
            stable_frames: 0,
        }
    }

    fn current(&self) -> QualityPreset {
        self.profile.presets[self.preset_index]
    }

    fn record_frame(&mut self, pressure: bool) -> bool {
        if pressure {
            self.stable_frames = 0;
            self.pressure_score = self.pressure_score.saturating_add(1);
            if self.pressure_score >= SCREEN_PRESSURE_THRESHOLD
                && self.preset_index + 1 < self.profile.presets.len()
            {
                self.preset_index += 1;
                self.pressure_score = 0;
                return true;
            }
            return false;
        }

        self.pressure_score = self.pressure_score.saturating_sub(1);
        self.stable_frames = self.stable_frames.saturating_add(1);
        let recovery_frames = (self.current().fps.max(8) * 6) as u16;
        if self.stable_frames >= recovery_frames && self.preset_index > 0 {
            self.preset_index -= 1;
            self.stable_frames = 0;
            self.pressure_score = 0;
            return true;
        }
        false
    }
}

fn screen_share_detail(
    preset: QualityPreset,
    transport_warning_active: bool,
    pacing_level: u8,
) -> String {
    let mut detail = preset.detail.to_string();
    if transport_warning_active {
        detail.push_str(
            " Viewer transport loss was detected, so share quality is staying conservative.",
        );
    }
    if pacing_level > 0 {
        detail.push_str(match pacing_level {
            1 => " Capture cadence is slightly throttled to reduce burst pressure.",
            _ => " Capture cadence is heavily throttled to protect viewer stability.",
        });
    }
    detail
}

fn enumerate_sources() -> Result<Vec<CaptureSource>, String> {
    let mut sources = enumerate_displays()?;
    match enumerate_windows() {
        Ok(mut windows) => sources.append(&mut windows),
        Err(e) => log::warn!("Failed to enumerate shareable windows: {e}"),
    }
    if sources.is_empty() {
        return Err("No display or window is available to share".to_string());
    }
    Ok(sources)
}

fn enumerate_displays() -> Result<Vec<CaptureSource>, String> {
    let monitors = Monitor::all().map_err(|e| format!("Failed to enumerate displays: {e}"))?;
    Ok(monitors
        .into_iter()
        .map(|monitor| {
            let id = monitor.id().unwrap_or_default();
            let name = monitor
                .friendly_name()
                .ok()
                .filter(|name| !name.trim().is_empty())
                .or_else(|| monitor.name().ok())
                .filter(|name| !name.trim().is_empty())
                .unwrap_or_else(|| format!("Display {id}"));
            let width = monitor.width().unwrap_or_default();
            let height = monitor.height().unwrap_or_default();
            let primary = monitor.is_primary().unwrap_or(false);
            let detail = if primary {
                format!("{width}x{height} · Primary display")
            } else {
                format!("{width}x{height} · Display capture")
            };
            CaptureSource {
                id: format!("display:{id}"),
                label: name,
                detail,
                kind_label: "Display".into(),
                target: CaptureTarget::Display(monitor),
            }
        })
        .collect())
}

fn enumerate_windows() -> Result<Vec<CaptureSource>, String> {
    let app_pid = std::process::id();
    let windows = Window::all().map_err(|e| format!("Failed to enumerate windows: {e}"))?;
    let mut sources = Vec::new();
    for window in windows {
        let pid = match window.pid() {
            Ok(pid) if pid != app_pid => pid,
            _ => continue,
        };
        if window.is_minimized().unwrap_or(false) {
            continue;
        }
        let width = window.width().unwrap_or_default();
        let height = window.height().unwrap_or_default();
        if width < 360 || height < 220 {
            log::debug!("Skipping window (pid {pid}): too small ({width}x{height} < 360x220)");
            continue;
        }
        let title = window.title().unwrap_or_default().trim().to_string();
        let app_name = window.app_name().unwrap_or_default().trim().to_string();
        if title.is_empty() && app_name.is_empty() {
            log::debug!("Skipping window (pid {pid}): empty title and app name");
            continue;
        }
        let id = match window.id() {
            Ok(id) => id,
            Err(_) => continue,
        };
        let label = if !title.is_empty() {
            title
        } else {
            app_name.clone()
        };
        let mut detail_parts = Vec::new();
        detail_parts.push(if app_name.is_empty() {
            "Window".to_string()
        } else {
            app_name
        });
        detail_parts.push(format!("{width}x{height}"));
        if window.is_focused().unwrap_or(false) {
            detail_parts.push("Focused".into());
        }
        sources.push(CaptureSource {
            id: format!("window:{pid}:{id}"),
            label,
            detail: detail_parts.join(" · "),
            kind_label: "Window".into(),
            target: CaptureTarget::Window(window),
        });
    }
    Ok(sources)
}

fn default_source_index(sources: &[CaptureSource], preferred_id: Option<&str>) -> Option<usize> {
    preferred_id
        .and_then(|id| sources.iter().position(|source| source.id == id))
        .or_else(|| {
            sources.iter().position(|source| {
                matches!(source.target, CaptureTarget::Window(_))
                    && source.detail.contains("Focused")
            })
        })
        .or_else(|| {
            sources.iter().position(|source| {
                matches!(source.target, CaptureTarget::Display(_))
                    && source.detail.contains("Primary")
            })
        })
        .or(if sources.is_empty() { None } else { Some(0) })
}

fn probe_source(source: &CaptureTarget) -> Result<(), String> {
    match source {
        CaptureTarget::Display(monitor) => monitor
            .capture_image()
            .map(|_| ())
            .map_err(|e| format!("Display capture is unavailable: {e}")),
        CaptureTarget::Window(window) => window
            .capture_image()
            .map(|_| ())
            .map_err(|e| format!("Window capture is unavailable: {e}")),
    }
}

fn capture_frame(source: &CaptureTarget) -> Result<xcap::image::RgbaImage, String> {
    match source {
        CaptureTarget::Display(monitor) => monitor
            .capture_image()
            .map_err(|e| format!("Display capture failed: {e}")),
        CaptureTarget::Window(window) => window
            .capture_image()
            .map_err(|e| format!("Window capture failed: {e}")),
    }
}

fn build_preview_frame(image: &xcap::image::RgbaImage, max_width: u32) -> PreviewFrame {
    let (width, height) = image.dimensions();
    let preview = if width > max_width {
        let target_height = ((height as u64 * max_width as u64) / width as u64).max(1) as u32;
        xcap::image::imageops::resize(image, max_width, target_height, FilterType::Triangle)
    } else {
        image.clone()
    };
    let (preview_width, preview_height) = preview.dimensions();
    PreviewFrame {
        width: preview_width,
        height: preview_height,
        pixels: preview.into_raw(),
    }
}

fn encode_frame(image: xcap::image::RgbaImage, preset: QualityPreset) -> Result<Vec<u8>, String> {
    let (width, height) = image.dimensions();
    let target = if width > preset.max_width {
        let target_height =
            ((height as u64 * preset.max_width as u64) / width as u64).max(1) as u32;
        xcap::image::imageops::resize(
            &image,
            preset.max_width,
            target_height,
            FilterType::Triangle,
        )
    } else {
        image
    };

    let mut encoded = Vec::new();
    let mut cursor = Cursor::new(&mut encoded);
    let mut encoder = JpegEncoder::new_with_quality(&mut cursor, preset.jpeg_quality);
    encoder
        .encode_image(&xcap::image::DynamicImage::ImageRgba8(target))
        .map_err(|e| format!("JPEG encode failed: {e}"))?;
    Ok(encoded)
}

#[cfg(test)]
mod tests {
    use super::{
        build_preview_frame, screen_share_detail, AdaptiveQuality, QualityPreset,
        TransportFeedbackSample, TransportFeedbackState, TransportFeedbackSummary, TransportPacer,
        SHARE_PROFILES,
    };
    use std::time::{Duration, Instant};
    use xcap::image::{Rgba, RgbaImage};

    #[test]
    fn adaptive_quality_steps_down_under_pressure() {
        let mut adaptive = AdaptiveQuality::new(SHARE_PROFILES[3]);
        assert_eq!(adaptive.current().name, "720p / 60 fps");
        assert!(!adaptive.record_frame(true));
        assert!(!adaptive.record_frame(true));
        assert!(adaptive.record_frame(true));
        assert_eq!(adaptive.current().name, "720p / 30 fps");
    }

    #[test]
    fn adaptive_quality_recovers_after_stability() {
        let mut adaptive = AdaptiveQuality::new(SHARE_PROFILES[2]);
        adaptive.record_frame(true);
        adaptive.record_frame(true);
        adaptive.record_frame(true);
        assert_eq!(adaptive.current().name, "720p / 20 fps");

        for _ in 0..180 {
            adaptive.record_frame(false);
        }
        assert_eq!(adaptive.current().name, "720p / 30 fps");
    }

    #[test]
    fn preview_frame_downscales_large_capture() {
        let image = RgbaImage::from_pixel(1920, 1080, Rgba([4, 8, 12, 255]));
        let preview = build_preview_frame(&image, 420);
        assert_eq!(preview.width, 420);
        assert_eq!(preview.height, 236);
        assert_eq!(
            preview.pixels.len(),
            (preview.width * preview.height * 4) as usize
        );
    }

    #[test]
    fn transport_feedback_summary_marks_pressure_on_loss() {
        let mut feedback = TransportFeedbackState::default();
        feedback.record(TransportFeedbackSample {
            frames_completed: 8,
            frames_dropped: 2,
            frames_timed_out: 0,
        });
        let summary = feedback.summary();
        assert!(summary.is_under_pressure());
    }

    #[test]
    fn transport_feedback_summary_prunes_stale_reports() {
        let mut feedback = TransportFeedbackState::default();
        feedback.record(TransportFeedbackSample {
            frames_completed: 4,
            frames_dropped: 0,
            frames_timed_out: 0,
        });
        if let Some(report) = feedback.reports.front_mut() {
            report.recorded_at =
                Instant::now() - super::SCREEN_TRANSPORT_FEEDBACK_WINDOW - Duration::from_millis(5);
        }
        let summary = feedback.summary();
        assert_eq!(summary.frames_completed, 0);
        assert!(!summary.is_under_pressure());
    }

    #[test]
    fn screen_share_detail_mentions_transport_loss() {
        let detail = screen_share_detail(
            QualityPreset {
                name: "test",
                max_width: 640,
                fps: 8,
                jpeg_quality: 50,
                detail: "Base detail.",
            },
            true,
            1,
        );
        assert!(detail.contains("Viewer transport loss"));
        assert!(detail.contains("slightly throttled"));
    }

    #[test]
    fn transport_pacer_steps_up_and_recovers() {
        let mut pacer = TransportPacer::default();
        assert_eq!(pacer.level(), 0);

        assert!(pacer.record_feedback(TransportFeedbackSummary {
            version: 1,
            frames_completed: 8,
            frames_dropped: 2,
            frames_timed_out: 0,
        }));
        assert_eq!(pacer.level(), 1);

        assert!(pacer.record_feedback(TransportFeedbackSummary {
            version: 2,
            frames_completed: 6,
            frames_dropped: 0,
            frames_timed_out: 1,
        }));
        assert_eq!(pacer.level(), 2);

        assert!(!pacer.record_feedback(TransportFeedbackSummary {
            version: 3,
            frames_completed: 10,
            frames_dropped: 0,
            frames_timed_out: 0,
        }));
        assert_eq!(pacer.level(), 2);

        assert!(pacer.record_feedback(TransportFeedbackSummary {
            version: 4,
            frames_completed: 12,
            frames_dropped: 0,
            frames_timed_out: 0,
        }));
        assert_eq!(pacer.level(), 1);
    }

    #[test]
    fn transport_pacer_applies_longer_frame_interval() {
        let mut pacer = TransportPacer::default();
        let base = Duration::from_millis(100);
        assert!(pacer.paced_interval(base) >= base);
        assert!(pacer.record_feedback(TransportFeedbackSummary {
            version: 1,
            frames_completed: 5,
            frames_dropped: 1,
            frames_timed_out: 0,
        }));
        assert!(pacer.paced_interval(base) > base);
    }
}
