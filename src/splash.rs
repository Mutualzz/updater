use std::sync::{Arc, Mutex};
use eframe::egui;
use egui::{Color32, FontId, Pos2, Rect, Rounding, Stroke, Vec2};
use log::info;

use crate::SplashCmd;

const FONT_REGULAR: &[u8] = include_bytes!("../resources/Rubik-Regular.ttf");
const FONT_BOLD:    &[u8] = include_bytes!("../resources/Rubik-Bold.ttf");

const BG:             Color32 = Color32::from_rgb(0x24, 0x19, 0x27);
const TEXT_PRIMARY:   Color32 = Color32::from_rgb(0xFF, 0xF7, 0xFB);
const TEXT_DIM:       Color32 = Color32::from_rgba_premultiplied(0xFF, 0xF7, 0xFB, 0x73);
const PROGRESS_TRACK: Color32 = Color32::from_rgba_premultiplied(0x6c, 0x5a, 0x6d, 0x59);
const PROGRESS_FILL:  Color32 = Color32::from_rgb(0x00, 0xD1, 0xC1);
const DOT_COLOR:      Color32 = Color32::from_rgb(0x6c, 0x5a, 0x6d);

const W: f32 = 300.0;
const H: f32 = 340.0;

#[derive(Default)]
struct SplashState {
    status:       String,
    progress:     Option<f64>,
    should_close: bool,
}

struct SplashApp {
    state:      Arc<Mutex<SplashState>>,
    logo:       Option<egui::TextureHandle>,
    logo_bytes: &'static [u8],
    dot_t:      f64,
    centered:   bool,
}

impl SplashApp {
    fn new(state: Arc<Mutex<SplashState>>) -> Self {
        Self {
            state,
            logo: None,
            logo_bytes: include_bytes!("../resources/icon.png"),
            dot_t: 0.0,
            centered: false,
        }
    }

    fn load_logo(&mut self, ctx: &egui::Context) {
        if self.logo.is_some() { return; }
        if let Ok(img) = image::load_from_memory(self.logo_bytes) {
            let rgba = img.to_rgba8();
            let (w, h) = rgba.dimensions();
            let pixels: Vec<Color32> = rgba
                .pixels()
                .map(|p| Color32::from_rgba_unmultiplied(p[0], p[1], p[2], p[3]))
                .collect();
            self.logo = Some(ctx.load_texture(
                "logo",
                egui::ColorImage { size: [w as usize, h as usize], pixels },
                egui::TextureOptions::LINEAR,
            ));
        }
    }
}

impl eframe::App for SplashApp {
    fn clear_color(&self, _visuals: &egui::Visuals) -> [f32; 4] {
        [0x24 as f32 / 255.0, 0x19 as f32 / 255.0, 0x27 as f32 / 255.0, 1.0]
    }

    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        self.load_logo(ctx);
        self.dot_t += ctx.input(|i| i.unstable_dt) as f64;

        if !self.centered {
            self.centered = true;
            if let Some(cmd) = egui::ViewportCommand::center_on_screen(ctx) {
                ctx.send_viewport_cmd(cmd);
            }
        }

        if ctx.input(|i| i.pointer.primary_pressed()) {
            ctx.send_viewport_cmd(egui::ViewportCommand::StartDrag);
        }

        let (status, progress, should_close) = {
            let s = self.state.lock().unwrap();
            (s.status.clone(), s.progress, s.should_close)
        };

        if should_close {
            ctx.send_viewport_cmd(egui::ViewportCommand::Close);
            return;
        }

        ctx.request_repaint_after(std::time::Duration::from_millis(16));

        egui::CentralPanel::default()
            .frame(egui::Frame::none())
            .show(ctx, |ui| {
                let full = ui.max_rect();
                let painter = ui.painter();

                painter.rect_filled(full, Rounding::same(16.0), BG);
                painter.rect_stroke(
                    full.shrink(0.5),
                    Rounding::same(16.0),
                    Stroke::new(1.0, Color32::from_rgba_premultiplied(0xFF, 0xF7, 0xFB, 0x14)),
                );

                let cx = full.center().x;
                let mut y = 70.0;

                let logo_rect = Rect::from_min_size(
                    Pos2::new(cx - 44.0, y),
                    Vec2::splat(88.0),
                );
                painter.rect_filled(
                    logo_rect.translate(Vec2::new(0.0, 6.0)).expand(1.0),
                    Rounding::same(24.0),
                    Color32::from_black_alpha(70),
                );
                if let Some(tex) = &self.logo {
                    painter.image(
                        tex.id(),
                        logo_rect,
                        Rect::from_min_max(Pos2::ZERO, Pos2::new(1.0, 1.0)),
                        Color32::WHITE,
                    );
                }
                y += 88.0 + 24.0;

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
                    let track = Rect::from_min_size(
                        Pos2::new(cx - tw / 2.0, y),
                        Vec2::new(tw, th),
                    );
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
                    let dot_r  = 2.0;
                    let gap    = 6.0;
                    let dots_y = full.max.y - 16.0 - dot_r;
                    let xs     = [cx - gap - dot_r * 2.0, cx, cx + gap + dot_r * 2.0];
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
                        let scale   = 1.0 + alpha * 0.3;
                        painter.circle_filled(
                            Pos2::new(x, dots_y),
                            dot_r * scale,
                            Color32::from_rgba_unmultiplied(
                                DOT_COLOR.r(), DOT_COLOR.g(), DOT_COLOR.b(),
                                (opacity * 255.0) as u8,
                            ),
                        );
                    }
                }
            });
    }
}

pub fn run(rx: std::sync::mpsc::Receiver<SplashCmd>) {
    let state = Arc::new(Mutex::new(SplashState {
        status: "Checking for updates...".into(),
        progress: None,
        should_close: false,
    }));

    let state_tx = Arc::clone(&state);
    std::thread::spawn(move || {
        while let Ok(cmd) = rx.recv() {
            let mut s = state_tx.lock().unwrap();
            match cmd {
                SplashCmd::SetStatus(text) => s.status = text,
                SplashCmd::SetProgress(pct) => s.progress = Some(pct),
                SplashCmd::HideProgress     => s.progress = None,
                SplashCmd::Close            => { s.should_close = true; break; }
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
        vsync: false,
        ..Default::default()
    };

    info!("Splash window open");

    eframe::run_native(
        "Mutualzz",
        options,
        Box::new(|cc| {
            let ctx = &cc.egui_ctx;
            let bg = Color32::from_rgb(0x24, 0x19, 0x27);
            let stroke = Stroke::new(1.0, bg);
            let mut visuals = egui::Visuals::dark();
            visuals.window_stroke                       = stroke;
            visuals.window_fill                         = bg;
            visuals.panel_fill                          = bg;
            visuals.extreme_bg_color                    = bg;
            visuals.faint_bg_color                      = bg;
            visuals.widgets.noninteractive.bg_fill      = bg;
            visuals.widgets.noninteractive.weak_bg_fill = bg;
            visuals.widgets.noninteractive.bg_stroke    = stroke;
            visuals.widgets.noninteractive.fg_stroke    = stroke;
            visuals.widgets.inactive.bg_fill            = bg;
            visuals.widgets.inactive.weak_bg_fill       = bg;
            visuals.widgets.inactive.bg_stroke          = stroke;
            visuals.widgets.hovered.bg_fill             = bg;
            visuals.widgets.hovered.weak_bg_fill        = bg;
            visuals.widgets.hovered.bg_stroke           = stroke;
            visuals.widgets.active.bg_fill              = bg;
            visuals.widgets.active.weak_bg_fill         = bg;
            visuals.widgets.active.bg_stroke            = stroke;
            visuals.widgets.open.bg_fill                = bg;
            visuals.widgets.open.weak_bg_fill           = bg;
            visuals.widgets.open.bg_stroke              = stroke;
            ctx.set_visuals(visuals);

            // Load Rubik font
            let mut fonts = egui::FontDefinitions::default();
            fonts.font_data.insert(
                "Rubik-Regular".into(),
                egui::FontData::from_static(FONT_REGULAR),
            );
            fonts.font_data.insert(
                "Rubik-Bold".into(),
                egui::FontData::from_static(FONT_BOLD),
            );
            fonts.families
                .entry(egui::FontFamily::Proportional)
                .or_default()
                .insert(0, "Rubik-Regular".into());
            fonts.families
                .entry(egui::FontFamily::Name("Bold".into()))
                .or_default()
                .push("Rubik-Bold".into());
            ctx.set_fonts(fonts);

            Ok(Box::new(SplashApp::new(state)))
        }),
    ).expect("Failed to run splash");
}