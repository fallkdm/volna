//! Питомец на панели плеера.
//!
//! Живёт поверх нижней части окна, гуляет по «полочке» над содержимым плеера
//! и прыгает в такт басу, который уже считает анализатор спектра.

use std::cell::RefCell;
use std::rc::Rc;
use std::sync::Arc;

use gtk4::prelude::*;
use gtk4::{glib, Align, DrawingArea};

use crate::player::audio::AudioBackend;
use crate::player::PlayerManager;

/// Облик питомца.
#[derive(Clone, Copy, PartialEq, Eq)]
enum PetStyle {
    Wave,
    Cat,
    Slime,
}

impl PetStyle {
    fn from_str(style: &str) -> Self {
        match style {
            "cat" => Self::Cat,
            "slime" => Self::Slime,
            _ => Self::Wave,
        }
    }
}

struct Heart {
    x: f64,
    y: f64,
    vx: f64,
    life: f64,
}

/// Слеза: координаты относительно глаз питомца.
struct Tear {
    x: f64,
    y: f64,
    vy: f64,
    life: f64,
}

/// Всё, что нужно для жизни питомца: положение, настроение и прыжки.
pub struct PetState {
    enabled: bool,
    style: PetStyle,
    /// Позиция по ширине плеера, 0..1
    x: f64,
    /// Направление прогулки: +1 вправо, -1 влево
    dir: f64,
    pause_t: f64,
    next_pause: f64,
    walk_phase: f64,
    hop: f64,
    hop_v: f64,
    squish: f64,
    blink_t: f64,
    blink: f64,
    time: f64,
    last_time: f64,
    bass: f64,
    last_bass: f64,
    beat_cd: f64,
    playing: bool,
    hearts: Vec<Heart>,
    /// Пасхалка: питомец забился в угол и плачет под «I Really Want to Stay at Your House»
    sad: bool,
    sad_corner: f64,
    sad_time: f64,
    tear_timer: f64,
    tears: Vec<Tear>,
    seed: u64,
}

impl PetState {
    fn new(enabled: bool, style: &str) -> Self {
        Self {
            enabled,
            style: PetStyle::from_str(style),
            x: 0.34,
            dir: 1.0,
            pause_t: 0.0,
            next_pause: 4.0,
            walk_phase: 0.0,
            hop: 0.0,
            hop_v: 0.0,
            squish: 0.0,
            blink_t: 2.5,
            blink: 0.0,
            time: 0.0,
            last_time: 0.0,
            bass: 0.0,
            last_bass: 0.0,
            beat_cd: 0.0,
            playing: false,
            hearts: Vec::new(),
            sad: false,
            sad_corner: 0.055,
            sad_time: 0.0,
            tear_timer: 0.0,
            tears: Vec::new(),
            seed: 0x9e3779b97f4a7c15,
        }
    }

    fn rand01(&mut self) -> f64 {
        self.seed ^= self.seed << 13;
        self.seed ^= self.seed >> 7;
        self.seed ^= self.seed << 17;
        (self.seed >> 11) as f64 / (1u64 << 53) as f64
    }

    fn set_enabled(&mut self, enabled: bool) {
        self.enabled = enabled;
        if enabled {
            self.last_time = 0.0;
            self.blink_t = 1.5;
        } else {
            self.hop = 0.0;
            self.hop_v = 0.0;
            self.squish = 0.0;
            self.hearts.clear();
            self.tears.clear();
            self.sad = false;
            self.sad_time = 0.0;
        }
    }

    fn set_style(&mut self, style: &str) {
        self.style = PetStyle::from_str(style);
    }

    fn update(&mut self, dt: f64, playing: bool, bass: f64, title: &str) {
        self.time += dt;
        self.playing = playing;

        // Пасхалка: «I Really Want to Stay at Your House» — питомец уходит в угол и плачет.
        let norm = title.to_lowercase();
        let sad_song = norm.contains("i really want to stay at your house")
            || norm.contains("i really wanna stay at your house")
            || norm.contains("stay at your house");
        let sad = playing && sad_song;
        if sad != self.sad {
            self.sad = sad;
            if sad {
                self.hop = 0.0;
                self.hop_v = 0.0;
                self.squish = 0.0;
                self.hearts.clear();
                self.tears.clear();
                self.sad_corner = if self.x < 0.5 { 0.055 } else { 0.945 };
                self.sad_time = 0.0;
                self.tear_timer = 0.2;
            } else {
                self.tears.clear();
            }
        }

        if self.sad {
            self.sad_time += dt;
            // Забиться в ближайший угол и сидеть там.
            let dx = self.sad_corner - self.x;
            if dx.abs() > 0.008 {
                self.dir = dx.signum();
                self.x += self.dir * 0.14 * dt;
                self.walk_phase += dt * 11.0;
            } else {
                self.x = self.sad_corner;
                self.walk_phase = 0.0;
                self.dir = if self.sad_corner < 0.5 { -1.0 } else { 1.0 };
            }

            self.tear_timer -= dt;
            if self.tear_timer <= 0.0 {
                let side = if self.tears.len() % 2 == 0 { -1.0 } else { 1.0 };
                let vy = 20.0 + self.rand01() * 16.0;
                self.tears.push(Tear {
                    x: side * 3.0,
                    y: 2.0,
                    vy,
                    life: 1.0,
                });
                self.tear_timer = 0.45 + self.rand01() * 0.45;
            }
            for tear in &mut self.tears {
                tear.life -= dt;
                tear.y += tear.vy * dt;
            }
            self.tears.retain(|tear| tear.life > 0.0);

            self.blink_t -= dt;
            if self.blink_t <= 0.0 {
                self.blink = 0.13;
                self.blink_t = 3.0 + self.rand01() * 3.0;
            }
            self.blink = (self.blink - dt).max(0.0);
            self.last_bass = bass;
            self.beat_cd = 0.4;
            return;
        }

        // Сглаживаем бас и ловим атаку: именно она даёт «прыжок в такт».
        let bass = bass.clamp(0.0, 1.0);
        self.bass += (bass - self.bass) * 0.35;
        let rising = bass - self.last_bass;
        self.last_bass = bass;
        self.beat_cd = (self.beat_cd - dt).max(0.0);

        if playing && self.beat_cd <= 0.0 && bass > 0.50 && rising > 0.045 {
            // Короткий и упругий подскок: не улетает в космос и не парит.
            self.hop_v = 150.0 + bass * 70.0;
            self.squish = 0.5;
            self.beat_cd = 0.22;
            self.spawn_heart();
        }

        // Прыжок с сильной гравитацией: приземляется быстро, без парения.
        if self.hop > 0.0 || self.hop_v > 0.0 {
            self.hop += self.hop_v * dt;
            self.hop_v -= 2200.0 * dt;
            if self.hop <= 0.0 {
                self.hop = 0.0;
                self.hop_v = 0.0;
                self.squish = 0.55;
            }
        }
        self.squish = (self.squish - dt * 7.0).max(0.0);

        if playing {
            if self.pause_t > 0.0 {
                self.pause_t -= dt;
            } else {
                self.next_pause -= dt;
                if self.next_pause <= 0.0 {
                    self.pause_t = 0.8 + self.rand01() * 1.8;
                    self.next_pause = 4.0 + self.rand01() * 6.0;
                } else {
                    // Спокойная прогулка, чуть быстрее на плотном басу.
                    let speed = 0.040 + self.bass * 0.045;
                    self.x += self.dir * speed * dt;
                    self.walk_phase += dt * (7.0 + self.bass * 5.0);
                    if self.x <= 0.05 {
                        self.x = 0.05;
                        self.dir = 1.0;
                    } else if self.x >= 0.95 {
                        self.x = 0.95;
                        self.dir = -1.0;
                    }
                }
            }
        } else {
            self.walk_phase = 0.0;
        }

        // Моргание и лёгкая ленивая пауза.
        self.blink_t -= dt;
        if self.blink_t <= 0.0 {
            self.blink = 0.13;
            self.blink_t = 2.4 + self.rand01() * 4.5;
        }
        self.blink = (self.blink - dt).max(0.0);

        for heart in &mut self.hearts {
            heart.life -= dt;
            heart.y -= 15.0 * dt;
            heart.x += heart.vx * dt;
        }
        self.hearts.retain(|heart| heart.life > 0.0);
    }

    fn spawn_heart(&mut self) {
        if self.hearts.len() >= 3 {
            return;
        }
        let vx = self.dir * (4.0 + self.rand01() * 7.0);
        self.hearts.push(Heart {
            x: 0.0,
            y: 0.0,
            vx,
            life: 1.25,
        });
    }
}

pub struct PetWidgets {
    pub layer: DrawingArea,
    pub state: Rc<RefCell<PetState>>,
    /// Тик анимации: снимается с frame clock на паузе
    pub tick_control: Rc<super::tick::TickControl>,
}

/// Создаёт прозрачный слой над нижней частью окна. Питомец сам находит
/// панель плеера через `compute_bounds`, поэтому слой не зависит от вёрстки.
pub fn create_pet_layer(
    island: &impl IsA<gtk4::Widget>,
    player: &PlayerManager,
    enabled: bool,
    style: &str,
) -> PetWidgets {
    let state = Rc::new(RefCell::new(PetState::new(enabled, style)));
    let audio: Arc<AudioBackend> = player.audio.clone();
    let player = player.clone();

    let layer = DrawingArea::new();
    layer.set_halign(Align::Fill);
    layer.set_valign(Align::End);
    layer.set_hexpand(true);
    layer.set_vexpand(false);
    layer.set_content_height(320);
    layer.set_can_target(false);
    layer.set_can_focus(false);
    layer.set_visible(enabled);
    layer.add_css_class("player-pet-layer");

    let island_weak = island.downgrade();
    {
        let state = state.clone();
        let island_weak = island_weak.clone();
        layer.set_draw_func(move |area, cr, _width, _height| {
            let st = state.borrow();
            if !st.enabled {
                return;
            }
            let Some(island) = island_weak.upgrade() else {
                return;
            };
            let Some(bounds) = island.compute_bounds(area) else {
                return;
            };

            let ix = bounds.x() as f64;
            let iw = bounds.width() as f64;
            let iy = bounds.y() as f64;
            if iw < 80.0 {
                return;
            }
            let margin = 42.0_f64.min(iw * 0.12);
            let cx = (ix + margin + st.x * (iw - margin * 2.0))
                .clamp(ix + 24.0, ix + iw - 24.0);

            // Питомец стоит НА плеере: лапки на верхней кромке панели.
            draw_pet(cr, st.style, cx, iy - 1.0, &st);
        });
    }

    let tick_control: Rc<super::tick::TickControl>;
    {
        let state = state.clone();
        let audio = audio.clone();
        let player = player.clone();
        let initially_playing = player.audio.is_playing();
        let layer_for_tick = layer.clone();

        let installer = move || {
            let state = state.clone();
            let audio = audio.clone();
            let player = player.clone();
            let area = layer_for_tick.clone();

            area.add_tick_callback(move |area, _clock| {
                if !state.borrow().enabled {
                    state.borrow_mut().last_time = 0.0;
                    return glib::ControlFlow::Continue;
                }

                let now = glib::monotonic_time() as f64 / 1_000_000.0;
                let mut st = state.borrow_mut();
                let dt = if st.last_time > 0.0 {
                    (now - st.last_time).clamp(0.0, 0.1)
                } else {
                    1.0 / 60.0
                };
                st.last_time = now;
                let playing = audio.is_playing();
                let bass = audio.bass_level() as f64;
                let title = player
                    .state
                    .lock()
                    .ok()
                    .and_then(|state| {
                        state.current_track.as_ref().map(|track| track.title.clone())
                    })
                    .unwrap_or_default();
                st.update(dt, playing, bass, &title);
                drop(st);

                area.queue_draw();
                glib::ControlFlow::Continue
            })
        };

        tick_control = super::tick::TickControl::new(installer);
        if enabled && initially_playing {
            tick_control.wake();
        } else {
            layer.queue_draw();
        }
    }

    PetWidgets {
        layer,
        state,
        tick_control,
    }
}

impl PetWidgets {
    /// Play — будим tick; пауза — один финальный кадр и сон.
    pub fn set_playing(&self, playing: bool, title: &str) {
        if !self.state.borrow().enabled {
            self.tick_control.sleep();
            return;
        }
        if playing {
            self.tick_control.wake();
            return;
        }
        {
            let mut state = self.state.borrow_mut();
            state.update(0.016, false, 0.0, title);
            state.last_time = 0.0;
        }
        self.layer.queue_draw();
        self.tick_control.sleep();
    }

    /// Один кадр при смене трека/настроек, пока tick спит.
    pub fn refresh(&self, title: &str) {
        if !self.state.borrow().enabled || self.tick_control.is_active() {
            return;
        }
        {
            let mut state = self.state.borrow_mut();
            state.update(0.016, false, 0.0, title);
            state.last_time = 0.0;
        }
        self.layer.queue_draw();
    }
}

/// Применяет настройки из диалога: включение и облик.
pub fn apply_settings(pet: &PetWidgets, enabled: bool, style: &str) {
    let playing = {
        let mut state = pet.state.borrow_mut();
        state.set_enabled(enabled);
        state.set_style(style);
        state.playing
    };
    pet.layer.set_visible(enabled);
    if enabled && playing {
        pet.tick_control.wake();
    } else {
        pet.tick_control.sleep();
    }
    pet.layer.queue_draw();
}

// ===================== Пиксельный питомец =====================

/// Телo без лапoк: 14×12 пикселей. Лапки рисуются oтдельнo и анимируются.
const BODY: [&str; 12] = [
    "....o....o....",
    "...obboobbo...",
    "..obbbbbbbbo..",
    ".obbbbbbbbbbo.",
    ".obbbbbbbbbbo.",
    ".obbbebbebbbo.",
    ".obbbebbebbbo.",
    ".obbbbbbbbbbo.",
    ".obbbbmmbbbbo.",
    ".obbbbbbbbbbo.",
    "..obbbbbbbbo..",
    "...obbbbbbo...",
];

const SLIME_BODY: [&str; 12] = [
    "..............",
    "..............",
    "....oooooo....",
    "...obbbbbbo...",
    "..obbbbbbbbo..",
    ".obbbbbbbbbbo.",
    ".obbbebbebbbo.",
    ".obbbebbebbbo.",
    ".obbbbbbbbbbo.",
    ".obbbbmmbbbbo.",
    ".obbbbbbbbbbo.",
    "..oooooooooo..",
];

/// 5×5 пиксельное сердечко, которое взлетает на бит.
const HEART: [&str; 5] = [
    ".p.p.",
    "ppppp",
    "ppppp",
    ".ppp.",
    "..p..",
];

/// Тучка над плачущим питомцем.
const CLOUD: [&str; 3] = [
    ".ccc.",
    "ccccc",
    ".ccc.",
];

#[derive(Clone, Copy)]
struct Palette {
    body: (f64, f64, f64),
    light: (f64, f64, f64),
    outline: (f64, f64, f64),
    eye: (f64, f64, f64),
    mouth: (f64, f64, f64),
    cheek: (f64, f64, f64),
}

fn palette(style: PetStyle) -> Palette {
    match style {
        PetStyle::Wave => Palette {
            // Тёплый терракотовый — в духе пиксельного маскота Claude Code.
            body: (0.85, 0.45, 0.32),
            light: (0.95, 0.65, 0.50),
            outline: (0.36, 0.16, 0.10),
            eye: (0.10, 0.06, 0.04),
            mouth: (0.35, 0.16, 0.10),
            cheek: (0.95, 0.55, 0.48),
        },
        PetStyle::Cat => Palette {
            body: (0.94, 0.62, 0.32),
            light: (1.0, 0.82, 0.55),
            outline: (0.38, 0.20, 0.08),
            eye: (0.10, 0.06, 0.04),
            mouth: (0.36, 0.18, 0.08),
            cheek: (0.97, 0.54, 0.50),
        },
        PetStyle::Slime => Palette {
            body: (0.43, 0.90, 0.60),
            light: (0.72, 0.98, 0.80),
            outline: (0.10, 0.38, 0.20),
            eye: (0.04, 0.14, 0.08),
            mouth: (0.08, 0.32, 0.16),
            cheek: (0.55, 0.95, 0.68),
        },
    }
}

fn color_for(ch: char, p: Palette) -> Option<(f64, f64, f64)> {
    match ch {
        'o' => Some(p.outline),
        'b' => Some(p.body),
        'h' => Some(p.light),
        'e' => Some(p.eye),
        'm' => Some(p.mouth),
        'p' => Some(p.cheek),
        _ => None,
    }
}

#[allow(unused_must_use)]
fn draw_sprite(
    cr: &gtk4::cairo::Context,
    rows: &[&str],
    left: f64,
    top: f64,
    scale: f64,
    p: Palette,
    blink: bool,
) {
    for (y, row) in rows.iter().enumerate() {
        for (x, ch) in row.chars().enumerate() {
            let ch = if blink && ch == 'e' { 'm' } else { ch };
            let Some(color) = color_for(ch, p) else {
                continue;
            };
            cr.set_source_rgb(color.0, color.1, color.2);
            cr.rectangle(left + x as f64 * scale, top + y as f64 * scale, scale, scale);
            cr.fill();
        }
    }
}

#[allow(unused_must_use)]
fn draw_pet(cr: &gtk4::cairo::Context, style: PetStyle, cx: f64, feet_y: f64, st: &PetState) {
    let p = palette(style);
    let scale = 3.0;
    let hop = st.hop.min(12.0);
    // В горе питомец мелко дрожит и сидит, прижавшись к кромке.
    let shiver = if st.sad {
        ((st.time * 28.0).sin() * 1.2).round()
    } else {
        0.0
    };
    let cx = cx + shiver;
    let sad_drop = if st.sad { 1.0 } else { 0.0 };
    let base_y = (feet_y - hop + sad_drop).round();
    let left = (cx - 7.0 * scale).round();

    // Пиксели должны оставаться пикселями.
    cr.set_antialias(gtk4::cairo::Antialias::None);

    // Тень на полочке: сжимается, когда питомец подпрыгивает.
    let shadow_w = if hop > 0.0 { 18.0 } else { 24.0 };
    cr.set_source_rgba(0.0, 0.0, 0.0, 0.32);
    cr.rectangle((cx - shadow_w / 2.0).round(), feet_y - 2.0, shadow_w, 2.0);
    cr.fill();

    // Лужица слёз, которая растёт, пока играет грустная песня.
    if st.sad {
        let puddle_w = (st.sad_time * 5.0).clamp(6.0, 28.0);
        cr.set_source_rgba(0.45, 0.78, 0.98, 0.45);
        cr.rectangle((cx - puddle_w / 2.0).round(), feet_y - 1.0, puddle_w, 2.0);
        cr.fill();
    }

    let body_top = base_y - 12.0 * scale;
    let rows: &[&str] = if style == PetStyle::Slime {
        &SLIME_BODY
    } else {
        &BODY
    };
    draw_sprite(cr, rows, left, body_top, scale, p, st.blink > 0.0);

    // У кота — ушки и хвост из отдельных пикселей.
    if style == PetStyle::Cat {
        for side in [-1.0_f64, 1.0] {
            let ex = (left + if side < 0.0 { 2.0 } else { 9.0 } * scale).round();
            let ey = body_top - 2.0 * scale;
            cr.set_source_rgb(p.outline.0, p.outline.1, p.outline.2);
            cr.rectangle(ex, ey, 3.0 * scale, 2.0 * scale);
            cr.fill();
            cr.set_source_rgb(p.body.0, p.body.1, p.body.2);
            cr.rectangle(ex + scale, ey + scale, scale, scale);
            cr.fill();
        }
        // Хвост покачивается.
        let sway = (st.time * 3.0).sin() * 2.0;
        cr.set_source_rgb(p.body.0, p.body.1, p.body.2);
        cr.rectangle(left + 14.0 * scale, body_top + 7.0 * scale + sway, scale, 3.0 * scale);
        cr.rectangle(left + 15.0 * scale, body_top + 5.0 * scale + sway, scale, 2.0 * scale);
    }

    // Клешни Волнёнка — привет пиксельному маскоту Claude Code.
    if style == PetStyle::Wave {
        for side in [-1.0_f64, 1.0] {
            let ax = (cx + side * 8.0 * scale - 1.5 * scale).round();
            let ay = (body_top + 5.0 * scale).round();
            cr.set_source_rgb(p.outline.0, p.outline.1, p.outline.2);
            cr.rectangle(ax, ay, 3.0 * scale, 4.0 * scale);
            cr.fill();
            cr.set_source_rgb(p.body.0, p.body.1, p.body.2);
            cr.rectangle(ax + scale, ay + scale, scale, 2.0 * scale);
            cr.fill();
        }
    }

    // Лапки: у слизня их нет.
    if st.sad && style != PetStyle::Slime {
        // Поджатые лапки: питомец забился в угол.
        for side in [-1.0_f64, 1.0] {
            let fx = (cx + side * 3.0 * scale - 1.5 * scale).round();
            let fy = (base_y - 2.0 * scale).round();
            cr.set_source_rgb(p.outline.0, p.outline.1, p.outline.2);
            cr.rectangle(fx, fy, 3.0 * scale, 2.0 * scale);
            cr.fill();
            cr.set_source_rgb(p.body.0, p.body.1, p.body.2);
            cr.rectangle(fx + scale, fy, scale, scale);
            cr.fill();
        }
    } else if style != PetStyle::Slime {
        let step = if st.playing && st.pause_t <= 0.0 {
            if st.walk_phase.sin() >= 0.0 { 1.0 } else { -1.0 }
        } else {
            0.0
        };
        let lifted = if hop > 0.0 { 2.0 } else { 0.0 };
        for (side, dy) in [(-1.0_f64, step), (1.0_f64, -step)] {
            let fx = (cx + side * 4.0 * scale - 1.5 * scale).round();
            let fy = (base_y - 2.0 * scale - dy * scale - lifted).round();
            cr.set_source_rgb(p.outline.0, p.outline.1, p.outline.2);
            cr.rectangle(fx, fy, 3.0 * scale, 2.0 * scale);
            cr.fill();
            cr.set_source_rgb(p.body.0, p.body.1, p.body.2);
            cr.rectangle(fx + scale, fy, scale, scale);
            cr.fill();
        }
    }

    // Грустная мордочка, слёзы и тучка.
    if st.sad {
        let eye_y = base_y - 5.4 * scale;
        let eye_dx = 1.55 * scale;

        // Брови домиком.
        cr.set_source_rgb(p.outline.0, p.outline.1, p.outline.2);
        for side in [-1.0_f64, 1.0] {
            let ex = cx + side * eye_dx;
            cr.rectangle(ex - 1.6 * scale, eye_y - 4.2 * scale, 2.0 * scale, scale);
            cr.rectangle(ex + 0.4 * scale, eye_y - 3.2 * scale, 2.0 * scale, scale);
        }
        cr.fill();

        // Грустный рот: уголки вниз.
        cr.set_source_rgb(p.body.0, p.body.1, p.body.2);
        cr.rectangle(cx - 2.6 * scale, eye_y + 6.0 * scale, 5.2 * scale, 3.0 * scale);
        cr.fill();
        cr.set_source_rgb(p.mouth.0, p.mouth.1, p.mouth.2);
        cr.rectangle(cx - 3.0 * scale, eye_y + 9.0 * scale, scale, scale);
        cr.rectangle(cx - 2.0 * scale, eye_y + 8.0 * scale, 4.0 * scale, scale);
        cr.rectangle(cx + 2.0 * scale, eye_y + 9.0 * scale, scale, scale);
        cr.fill();

        // Слёзы.
        cr.set_source_rgba(0.45, 0.80, 1.0, 0.95);
        for tear in &st.tears {
            cr.rectangle(
                (cx + tear.x).round(),
                (eye_y + tear.y * scale).round(),
                2.0,
                3.0,
            );
            cr.fill();
        }

        // Тучка с дождём над головой.
        let cloud_scale = 3.0;
        let cloud_left = (cx - 2.5 * cloud_scale).round();
        let cloud_top = (body_top - 9.0 * cloud_scale).round();
        for (ry, row) in CLOUD.iter().enumerate() {
            for (rx, ch) in row.chars().enumerate() {
                if ch != 'c' {
                    continue;
                }
                cr.set_source_rgba(0.62, 0.74, 0.88, 0.95);
                cr.rectangle(
                    cloud_left + rx as f64 * cloud_scale,
                    cloud_top + ry as f64 * cloud_scale,
                    cloud_scale,
                    cloud_scale,
                );
                cr.fill();
            }
        }
        cr.set_source_rgba(0.45, 0.80, 1.0, 0.85);
        for i in 0..3 {
            let rx = cloud_left + (1.0 + i as f64 * 1.5) * cloud_scale;
            let ry = cloud_top + 3.0 * cloud_scale
                + ((st.time * 26.0 + i as f64 * 5.0) % 12.0);
            cr.rectangle(rx.round(), ry.round(), 2.0, 3.0);
            cr.fill();
        }
    }

    // Сердечки на бит.
    for heart in &st.hearts {
        let alpha = (heart.life / 1.25).clamp(0.0, 1.0) * 0.95;
        let hx = (cx + heart.x - 2.5 * scale).round();
        let hy = (body_top - 5.0 * scale + heart.y).round();
        for (y, row) in HEART.iter().enumerate() {
            for (x, ch) in row.chars().enumerate() {
                if ch != 'p' {
                    continue;
                }
                cr.set_source_rgba(p.cheek.0, p.cheek.1, p.cheek.2, alpha);
                cr.rectangle(hx + x as f64 * scale, hy + y as f64 * scale, scale, scale);
                cr.fill();
            }
        }
    }
}
