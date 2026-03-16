use std::io::Cursor;
use std::rc::Rc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use tokio::sync::{mpsc, Mutex as TokioMutex};
use ui_shell::{MainWindow, ShareSourceData};
use xcap::image::codecs::jpeg::JpegEncoder;
use xcap::image::imageops::FilterType;
use xcap::{Monitor, Window};

const SCREEN_SHARE_QUEUE_CAPACITY: usize = 2;
const SCREEN_PRESSURE_THRESHOLD: u8 = 3;

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
    stop_flag: Mutex<Option<Arc<AtomicBool>>>,
    selected_source_id: Mutex<Option<String>>,
    selected_profile_index: Mutex<usize>,
    cached_sources: Mutex<Vec<ScreenShareSourceDescriptor>>,
    ui_status: Mutex<ShareUiStatus>,
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

        let stop = Arc::new(AtomicBool::new(false));
        let stop_for_thread = stop.clone();
        let (tx, mut rx) = mpsc::channel::<Vec<u8>>(SCREEN_SHARE_QUEUE_CAPACITY);
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
        rt_handle.spawn(async move {
            while let Some(frame) = rx.recv().await {
                let net = network.lock().await;
                if let Err(e) = net.send_screen_frame(&frame).await {
                    log::warn!("Failed to send screen frame: {e}");
                }
            }
        });

        std::thread::spawn(move || {
            let mut adaptive = AdaptiveQuality::new(profile);
            while !stop_for_thread.load(Ordering::Relaxed) {
                let preset = adaptive.current();
                if let Ok(mut state) = status_for_thread.lock() {
                    state.quality_label = format!("{} · {}", profile.name, preset.name);
                    state.quality_detail = preset.detail.into();
                }

                let frame_interval = Duration::from_millis(1000 / preset.fps.max(1));
                let frame_start = Instant::now();
                let mut pressure = false;

                match capture_frame(&source.target) {
                    Ok(image) => match encode_frame(image, preset) {
                        Ok(encoded) => {
                            if encoded.len()
                                > shared_types::MAX_SCREEN_FRAME_SIZE.saturating_mul(3) / 4
                            {
                                pressure = true;
                            }
                            if tx.try_send(encoded).is_err() {
                                pressure = true;
                            }
                        }
                        Err(e) => {
                            pressure = true;
                            log::warn!("Failed to encode screen frame: {e}");
                        }
                    },
                    Err(e) => {
                        pressure = true;
                        log::warn!("Failed to capture screen frame: {e}");
                    }
                }

                let elapsed = frame_start.elapsed();
                if elapsed >= frame_interval.mul_f32(0.90) {
                    pressure = true;
                }
                let preset_changed = adaptive.record_frame(pressure);
                if preset_changed {
                    let active = adaptive.current();
                    if let Ok(mut state) = status_for_thread.lock() {
                        state.quality_label = format!("{} · {}", profile.name, active.name);
                        state.quality_detail = active.detail.into();
                    }
                }

                if elapsed < frame_interval {
                    std::thread::sleep(frame_interval - elapsed);
                }
            }
        });

        *state = Some(stop);
        Ok(())
    }

    pub fn stop_capture(&self) {
        if let Ok(mut state) = self.stop_flag.lock() {
            if let Some(stop) = state.take() {
                stop.store(true, Ordering::Relaxed);
            }
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
            continue;
        }
        let title = window.title().unwrap_or_default().trim().to_string();
        let app_name = window.app_name().unwrap_or_default().trim().to_string();
        if title.is_empty() && app_name.is_empty() {
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
    use super::{AdaptiveQuality, SHARE_PROFILES};

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
}
