use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use eframe::egui;
use egui::{Color32, FontId, Pos2, Rect, Rounding, Stroke, Vec2};
use log::info;

use crate::SplashCmd;

const FONT_REGULAR: &[u8] = include_bytes!("../resources/Rubik-Regular.ttf");
const FONT_BOLD: &[u8] = include_bytes!("../resources/Rubik-Bold.ttf");

const BG: Color32 = Color32::from_rgb(0x24, 0x19, 0x27);
const LOGO_PURPLE: Color32 = Color32::from_rgb(0x88, 0x44, 0x9a);
const TEXT_PRIMARY: Color32 = Color32::from_rgb(0xFF, 0xF7, 0xFB);
const TEXT_DIM: Color32 = Color32::from_rgba_premultiplied(0xFF, 0xF7, 0xFB, 0x73);
const PROGRESS_TRACK: Color32 = Color32::from_rgba_premultiplied(0x6c, 0x5a, 0x6d, 0x59);
const PROGRESS_FILL: Color32 = Color32::from_rgb(0x00, 0xD1, 0xC1);
const DOT_COLOR: Color32 = Color32::from_rgb(0x6c, 0x5a, 0x6d);

const W: f32 = 300.0;
const H: f32 = 340.0;
const CORNER: f32 = 16.0;
const LOGO_SIZE: f32 = 108.0;
const ICON_HOLD: f64 = 1.5;
const ICON_MOVE: f64 = 0.55;

const PENTAGRAM_BYTES: &[u8] = include_bytes!("../resources/logo/pentagram_overlay.png");
const CENTER_ICON: (&str, &[u8]) = ("anarchy", include_bytes!("../resources/logo/anarchy.png"));
const ORBIT_ICONS: &[(&str, &[u8])] = &[
    ("cathedral", include_bytes!("../resources/logo/cathedral.png")),
    ("scene_hair", include_bytes!("../resources/logo/scene_hair.png")),
    ("guitar", include_bytes!("../resources/logo/guitar.png")),
    ("microphone", include_bytes!("../resources/logo/microphone.png")),
    ("emo_hair", include_bytes!("../resources/logo/emo_hair.png")),
];

#[derive(Default)]
struct SplashState {
    status: String,
    progress: Option<f64>,
    should_close: bool,
    allow_skip: bool,
}

struct SplashApp {
    state: Arc<Mutex<SplashState>>,
    skip_launch: Option<Arc<AtomicBool>>,
    pentagram: Option<egui::TextureHandle>,
    center_icon: Option<egui::TextureHandle>,
    orbit_icons: Vec<egui::TextureHandle>,
    textures_loaded: bool,
    logo_t: f64,
    dot_t: f64,
    space_held: f64,
    centered: bool,
}

impl SplashApp {
    fn new(state: Arc<Mutex<SplashState>>, skip_launch: Option<Arc<AtomicBool>>) -> Self {
        Self {
            state,
            skip_launch,
            pentagram: None,
            center_icon: None,
            orbit_icons: Vec::new(),
            textures_loaded: false,
            logo_t: 0.0,
            dot_t: 0.0,
            space_held: 0.0,
            centered: false,
        }
    }

    fn load_textures(&mut self, ctx: &egui::Context) {
        if self.textures_loaded {
            return;
        }
        self.textures_loaded = true;
        self.pentagram = load_keyed_texture(ctx, "pentagram", PENTAGRAM_BYTES);
        self.center_icon = load_keyed_texture(ctx, CENTER_ICON.0, CENTER_ICON.1);
        self.orbit_icons = ORBIT_ICONS
            .iter()
            .filter_map(|(name, bytes)| load_keyed_texture(ctx, name, bytes))
            .collect();
    }

    fn draw_logo(&self, painter: &egui::Painter, rect: Rect) {
        let center = rect.center();
        let radius = rect.width() * 0.5;

        painter.circle_filled(center, radius, LOGO_PURPLE);

        let count = self.orbit_icons.len();
        if count > 0 {
            let cycle = ICON_HOLD + ICON_MOVE;
            let t = self.logo_t % cycle;
            let steps = (self.logo_t / cycle).floor() as i32;
            let spin = if t < ICON_HOLD {
                0.0
            } else {
                let p = ((t - ICON_HOLD) / ICON_MOVE).clamp(0.0, 1.0) as f32;
                p * p * (3.0 - 2.0 * p)
            };

            let slot_step = std::f32::consts::TAU / count as f32;
            let base_angle = -std::f32::consts::FRAC_PI_2;
            let orbit_r = radius * 0.698;
            let icon_size = radius * 0.26;

            for (i, tex) in self.orbit_icons.iter().enumerate() {
                let slot = (i as i32 + steps).rem_euclid(count as i32) as f32;
                let angle = base_angle + (slot + spin) * slot_step;
                let pos = Pos2::new(
                    center.x + orbit_r * angle.cos(),
                    center.y + orbit_r * angle.sin(),
                );
                painter.image(
                    tex.id(),
                    Rect::from_center_size(pos, Vec2::splat(icon_size)),
                    Rect::from_min_max(Pos2::ZERO, Pos2::new(1.0, 1.0)),
                    Color32::WHITE,
                );
            }
        }

        if let Some(tex) = &self.center_icon {
            let size = radius * 0.251;
            painter.image(
                tex.id(),
                Rect::from_center_size(center, Vec2::splat(size)),
                Rect::from_min_max(Pos2::ZERO, Pos2::new(1.0, 1.0)),
                Color32::WHITE,
            );
        }

        if let Some(tex) = &self.pentagram {
            painter.image(
                tex.id(),
                rect,
                Rect::from_min_max(Pos2::ZERO, Pos2::new(1.0, 1.0)),
                Color32::WHITE,
            );
        }
    }
}
fn load_keyed_texture(
    ctx: &egui::Context,
    name: &str,
    bytes: &[u8],
) -> Option<egui::TextureHandle> {
    let img = image::load_from_memory(bytes).ok()?.to_rgba8();
    let (w, h) = img.dimensions();
    let has_alpha = img.pixels().any(|p| p.0[3] < 255);

    let pixels: Vec<Color32> = img
        .pixels()
        .map(|p| {
            let [r, g, b, a] = p.0;
            if has_alpha {
                if a < 8 {
                    return Color32::TRANSPARENT;
                }
                let luma = (r as u16 + g as u16 + b as u16) / 3;
                if luma < 20 {
                    return Color32::TRANSPARENT;
                }
                return Color32::from_rgba_unmultiplied(255, 255, 255, a);
            }

            let luma = (r as u16 + g as u16 + b as u16) / 3;
            if luma < 28 {
                Color32::TRANSPARENT
            } else {
                let alpha = ((luma as u16 * a as u16) / 255) as u8;
                Color32::from_rgba_unmultiplied(255, 255, 255, alpha)
            }
        })
        .collect();

    Some(ctx.load_texture(
        name,
        egui::ColorImage {
            size: [w as usize, h as usize],
            pixels,
        },
        egui::TextureOptions {
            magnification: egui::TextureFilter::Linear,
            minification: egui::TextureFilter::Linear,
            wrap_mode: egui::TextureWrapMode::ClampToEdge,
            mipmap_mode: None,
        },
    ))
}

impl eframe::App for SplashApp {
    fn clear_color(&self, _visuals: &egui::Visuals) -> [f32; 4] {
        [0.0, 0.0, 0.0, 0.0]
    }

    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        self.load_textures(ctx);
        let dt = ctx.input(|i| i.unstable_dt) as f64;
        self.logo_t += dt;
        self.dot_t += dt;

        if !self.centered {
            self.centered = true;
            if let Some(cmd) = egui::ViewportCommand::center_on_screen(ctx) {
                ctx.send_viewport_cmd(cmd);
            }
        }

        if ctx.input(|i| i.pointer.primary_pressed()) {
            ctx.send_viewport_cmd(egui::ViewportCommand::StartDrag);
        }

        let (status, progress, should_close, allow_skip) = {
            let s = self.state.lock().unwrap();
            (s.status.clone(), s.progress, s.should_close, s.allow_skip)
        };

        if allow_skip {
            if ctx.input(|i| i.key_down(egui::Key::Space)) {
                self.space_held += dt;
                if self.space_held >= 0.45 {
                    if let Some(flag) = &self.skip_launch {
                        flag.store(true, Ordering::SeqCst);
                    }
                }
            } else {
                self.space_held = 0.0;
            }
        } else {
            self.space_held = 0.0;
        }

        if should_close {
            ctx.send_viewport_cmd(egui::ViewportCommand::Close);
            return;
        }

        ctx.request_repaint_after(std::time::Duration::from_millis(16));

        egui::CentralPanel::default()
            .frame(egui::Frame::none().fill(Color32::TRANSPARENT))
            .show(ctx, |ui| {
                let full = ui.max_rect();
                let panel = full.shrink(1.0);
                let painter = ui.painter();

                painter.rect_filled(panel, Rounding::same(CORNER), BG);

                let cx = panel.center().x;
                let mut y = panel.min.y + 64.0;

                let logo_rect =
                    Rect::from_min_size(Pos2::new(cx - LOGO_SIZE * 0.5, y), Vec2::splat(LOGO_SIZE));
                self.draw_logo(painter, logo_rect);
                y += LOGO_SIZE + 22.0;

                painter.text(
                    Pos2::new(cx, y),
                    egui::Align2::CENTER_TOP,
                    "Mutualzz",
                    FontId::new(22.0, egui::FontFamily::Name("Bold".into())),
                    TEXT_PRIMARY,
                );
                y += 30.0 + 8.0;

                painter.text(
                    Pos2::new(cx, y),
                    egui::Align2::CENTER_TOP,
                    &status,
                    FontId::new(12.0, egui::FontFamily::Proportional),
                    TEXT_DIM,
                );
                y += 18.0 + 28.0;

                if let Some(pct) = progress {
                    let tw = 200.0;
                    let th = 4.0;
                    let track =
                        Rect::from_min_size(Pos2::new(cx - tw / 2.0, y), Vec2::new(tw, th));
                    painter.rect_filled(track, Rounding::same(999.0), PROGRESS_TRACK);
                    let fw = (tw * (pct as f32 / 100.0)).clamp(0.0, tw);
                    if fw > 0.0 {
                        let fill = Rect::from_min_size(track.min, Vec2::new(fw, th));
                        painter.rect_filled(
                            fill.expand(1.0),
                            Rounding::same(999.0),
                            Color32::from_rgba_premultiplied(0x00, 0xD1, 0xC1, 0x28),
                        );
                        painter.rect_filled(fill, Rounding::same(999.0), PROGRESS_FILL);
                    }
                } else {
                    let dot_r = 2.0;
                    let gap = 6.0;
                    let dots_y = panel.max.y - 16.0 - dot_r;
                    let xs = [cx - gap - dot_r * 2.0, cx, cx + gap + dot_r * 2.0];
                    let delays = [0.0_f64, 0.2, 0.4];
                    for (&x, &delay) in xs.iter().zip(delays.iter()) {
                        let phase = ((self.dot_t - delay).max(0.0) % 1.4) / 1.4;
                        let alpha = if phase < 0.4 {
                            phase / 0.4
                        } else if phase < 0.8 {
                            1.0 - (phase - 0.4) / 0.4
                        } else {
                            0.0
                        } as f32;
                        let opacity = (0.25 + alpha * 0.75).clamp(0.0, 1.0);
                        let scale = 1.0 + alpha * 0.3;
                        painter.circle_filled(
                            Pos2::new(x, dots_y),
                            dot_r * scale,
                            Color32::from_rgba_unmultiplied(
                                DOT_COLOR.r(),
                                DOT_COLOR.g(),
                                DOT_COLOR.b(),
                                (opacity * 255.0) as u8,
                            ),
                        );
                    }
                }
            });
    }
}

pub fn run(rx: std::sync::mpsc::Receiver<SplashCmd>, skip_launch: Option<Arc<AtomicBool>>) {
    let state = Arc::new(Mutex::new(SplashState {
        status: "Checking for updates...".into(),
        progress: None,
        should_close: false,
        allow_skip: false,
    }));

    let state_tx = Arc::clone(&state);
    std::thread::spawn(move || {
        while let Ok(cmd) = rx.recv() {
            let mut s = state_tx.lock().unwrap();
            match cmd {
                SplashCmd::SetStatus(text) => s.status = text,
                SplashCmd::SetProgress(pct) => s.progress = Some(pct),
                SplashCmd::HideProgress => s.progress = None,
                SplashCmd::SetAllowSkip(allow) => s.allow_skip = allow,
                SplashCmd::Close => {
                    s.should_close = true;
                    break;
                }
            }
        }
    });

    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_inner_size([W, H])
            .with_resizable(false)
            .with_decorations(false)
            .with_transparent(true)
            .with_always_on_top()
            .with_taskbar(false),
        centered: true,
        vsync: true,
        ..Default::default()
    };

    info!("Splash window open");

    eframe::run_native(
        "Mutualzz",
        options,
        Box::new(|cc| {
            let ctx = &cc.egui_ctx;
            let mut visuals = egui::Visuals::dark();
            visuals.window_fill = Color32::TRANSPARENT;
            visuals.panel_fill = Color32::TRANSPARENT;
            visuals.extreme_bg_color = Color32::TRANSPARENT;
            visuals.faint_bg_color = Color32::TRANSPARENT;
            visuals.window_stroke = Stroke::NONE;
            visuals.widgets.noninteractive.bg_fill = Color32::TRANSPARENT;
            visuals.widgets.noninteractive.weak_bg_fill = Color32::TRANSPARENT;
            visuals.widgets.noninteractive.bg_stroke = Stroke::NONE;
            visuals.widgets.inactive.bg_fill = Color32::TRANSPARENT;
            visuals.widgets.inactive.weak_bg_fill = Color32::TRANSPARENT;
            visuals.widgets.inactive.bg_stroke = Stroke::NONE;
            visuals.widgets.hovered.bg_fill = Color32::TRANSPARENT;
            visuals.widgets.hovered.weak_bg_fill = Color32::TRANSPARENT;
            visuals.widgets.hovered.bg_stroke = Stroke::NONE;
            visuals.widgets.active.bg_fill = Color32::TRANSPARENT;
            visuals.widgets.active.weak_bg_fill = Color32::TRANSPARENT;
            visuals.widgets.active.bg_stroke = Stroke::NONE;
            visuals.widgets.open.bg_fill = Color32::TRANSPARENT;
            visuals.widgets.open.weak_bg_fill = Color32::TRANSPARENT;
            visuals.widgets.open.bg_stroke = Stroke::NONE;
            ctx.set_visuals(visuals);

            let mut fonts = egui::FontDefinitions::default();
            fonts.font_data.insert(
                "Rubik-Regular".into(),
                egui::FontData::from_static(FONT_REGULAR),
            );
            fonts
                .font_data
                .insert("Rubik-Bold".into(), egui::FontData::from_static(FONT_BOLD));
            fonts
                .families
                .entry(egui::FontFamily::Proportional)
                .or_default()
                .insert(0, "Rubik-Regular".into());
            fonts
                .families
                .entry(egui::FontFamily::Name("Bold".into()))
                .or_default()
                .push("Rubik-Bold".into());
            ctx.set_fonts(fonts);

            Ok(Box::new(SplashApp::new(state, skip_launch)))
        }),
    )
    .expect("Failed to run splash");
}
