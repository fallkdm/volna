//! Фон Волны на видеокарте: тот же рисунок, что был на Cairo (заливка, свечение у Play, семь текучих нитей)
//! плюс плывущие тёмные пятна, но считается шейдером для каждого пикселя. Процессор только передаёт несколько чисел за кадр.

use std::cell::{Cell, RefCell};
use std::rc::Rc;

use glow::HasContext;
use gtk4::prelude::*;
use gtk4::GLArea;

use super::wave_view::WaveShaderState;

const VERTEX: &str = r#"
// треугольник на весь экран без буфера вершин
void main() {
    vec2 p = vec2(float((gl_VertexID << 1) & 2), float(gl_VertexID & 2));
    gl_Position = vec4(p * 2.0 - 1.0, 0.0, 1.0);
}
"#;

const FRAGMENT: &str = r#"
uniform vec2 u_size;    // логический размер виджета
uniform float u_scale;  // пикселей устройства на логический пиксель
uniform vec2 u_center;  // центр кнопки Play, логические координаты (y вниз)
uniform float u_time;
uniform float u_bass;
uniform vec3 u_core;
uniform vec3 u_deep;
uniform vec3 u_ridge;
out vec4 frag;

float hash(vec2 p) {
    return fract(sin(dot(p, vec2(127.1, 311.7))) * 43758.5453);
}

// гладкий шум значений и его сумма октав — рваный, «дымный» край тёмных пятен
float noise(vec2 p) {
    vec2 i = floor(p);
    vec2 f = fract(p);
    vec2 u = f * f * (3.0 - 2.0 * f);
    return mix(mix(hash(i), hash(i + vec2(1.0, 0.0)), u.x),
               mix(hash(i + vec2(0.0, 1.0)), hash(i + vec2(1.0, 1.0)), u.x), u.y);
}

float fbm(vec2 p) {
    float v = 0.0;
    float a = 0.5;
    for (int i = 0; i < 4; i++) {
        v += a * noise(p);
        p *= 2.03;
        a *= 0.5;
    }
    return v;
}

// покрытие линии толщиной 2*half (логические пиксели) со сглаживанием в один пиксель устройства
float stroke(float e, float half_width) {
    return clamp((half_width - e) * u_scale + 0.5, 0.0, 1.0);
}

void main() {
    vec2 p = vec2(gl_FragCoord.x, u_size.y * u_scale - gl_FragCoord.y) / u_scale;
    float maxd = max(u_size.x, u_size.y);
    vec3 base = u_core * 0.55 + u_deep * 0.45;

    // 1. сплошная радиальная заливка цветом трека
    float t = length(p - u_center) / (maxd * 0.95);
    vec3 c0 = clamp(base * 1.25 + u_ridge * 0.10, 0.0, 1.0);
    vec3 col = t < 0.55 ? mix(c0, base, t / 0.55) : mix(base, base * 0.45, clamp((t - 0.55) / 0.45, 0.0, 1.0));

    // мягкий свет у кнопки Play, усиливается басом
    float g = length(p - u_center) / (maxd * 0.42 * (1.0 + u_bass * 0.35));
    if (g < 1.0) {
        vec4 s0 = vec4(u_ridge, 0.38 + u_bass * 0.40);
        vec4 s1 = vec4(u_core, 0.14 + u_bass * 0.12);
        vec4 s2 = vec4(u_core, 0.0);
        vec4 s = g < 0.45 ? mix(s0, s1, g / 0.45) : mix(s1, s2, (g - 0.45) / 0.55);
        col = mix(col, s.rgb, clamp(s.a, 0.0, 1.0));
    }

    // темнота медленно «съедает» цвет: четыре мягких тёмных пятна с рваным краем плывут по экрану;
    // у кнопки Play цвет остаётся, чтобы центр не проваливался
    vec2 q = p / maxd;
    vec2 span = u_size / maxd;
    float dark = 0.0;
    for (int k = 0; k < 4; k++) {
        float kf = float(k);
        vec2 c = span * (0.5 + 0.48 * vec2(sin(u_time * 0.045 * (1.0 + kf * 0.3) + kf * 2.1),
                                           cos(u_time * 0.037 * (1.0 + kf * 0.25) + kf * 1.3)));
        float r = 0.30 + 0.06 * sin(u_time * 0.07 + kf * 1.7);
        float warp = fbm(q * 3.0 + vec2(u_time * 0.03, kf * 7.0)) - 0.5;
        float d = length(q - c) + warp * 0.20;
        dark = max(dark, 1.0 - smoothstep(r * 0.2, r, d));
    }
    float guard = smoothstep(0.06, 0.30, length(p - u_center) / maxd);
    col = mix(col, u_deep * 0.30, 0.68 * dark * guard);

    // 2. текучие нити: замкнутые кривые r(θ) вокруг центра; расстояние до кривой — по радиусу,
    // поправленное на наклон кривой, чтобы толщина не менялась на изгибах
    vec3 light = u_ridge * 0.5 + 0.5;
    for (int k = 0; k < 7; k++) {
        float kf = float(k);
        float frac = kf / 7.0;
        float radius = maxd * (0.14 + 0.14 * kf);
        vec2 drift = vec2(sin(u_time * 0.17 + kf * 1.9) * 36.0, cos(u_time * 0.13 + kf * 1.1) * 24.0);
        float speed = 1.0 + kf * 0.09;
        vec2 d = p - (u_center + drift);
        float dist = length(d);
        float th = atan(d.y, d.x);
        float a1 = th * 3.0 + u_time * 0.50 * speed + kf * 1.3;
        float a2 = th * 5.0 - u_time * 0.40 * speed + kf * 0.7;
        float a3 = th * 2.0 + u_time * 0.30 * speed + kf;
        float wave = 0.16 * sin(a1) + 0.10 * cos(a2) + 0.06 * sin(a3);
        float dwave = 0.48 * cos(a1) - 0.50 * sin(a2) + 0.12 * cos(a3);
        float r = radius * (1.0 + wave) + u_bass * 22.0 * (0.4 + frac);
        float dr = radius * dwave;
        float e = abs(dist - r) / sqrt(1.0 + dr * dr / (r * r));
        float strength = (0.32 + u_bass * 0.70) * (1.0 - frac * 0.40);
        col = mix(col, light, min(0.30 * strength, 1.0) * stroke(e, 6.0));
        col = mix(col, light, min(strength, 1.0) * stroke(e, 1.6));
    }

    frag = vec4(col, 1.0);
}
"#;

/// Объекты OpenGL, живут между realize и unrealize
struct Gl {
    gl: glow::Context,
    program: glow::Program,
    vao: glow::VertexArray,
}

/// Достаёт функции OpenGL через EGL (GTK 4 работает через EGL и на Wayland, и на X11)
fn load_gl() -> Option<glow::Context> {
    // VOLNA_NO_GL=1 имитирует отсутствие OpenGL: так проверяют запасной фон
    if std::env::var_os("VOLNA_NO_GL").is_some() {
        return None;
    }
    // shortcut: только EGL; если GTK запущен с GLX (GDK_DEBUG=gl-glx), фон не нарисуется
    let lib = unsafe { libloading::Library::new("libEGL.so.1") }.ok()?;
    let get: libloading::Symbol<unsafe extern "C" fn(*const std::ffi::c_char) -> *const std::ffi::c_void> =
        unsafe { lib.get(b"eglGetProcAddress\0") }.ok()?;
    let ctx = unsafe {
        glow::Context::from_loader_function(|name| {
            let name = std::ffi::CString::new(name).unwrap();
            get(name.as_ptr())
        })
    };
    std::mem::forget(lib); // функции нужны до конца работы приложения
    Some(ctx)
}

fn compile(gl: &glow::Context, es: bool) -> Result<(glow::Program, glow::VertexArray), String> {
    let header = if es { "#version 300 es\nprecision highp float;\n" } else { "#version 330 core\n" };
    unsafe {
        let program = gl.create_program()?;
        for (kind, src) in [(glow::VERTEX_SHADER, VERTEX), (glow::FRAGMENT_SHADER, FRAGMENT)] {
            let shader = gl.create_shader(kind)?;
            gl.shader_source(shader, &format!("{header}{src}"));
            gl.compile_shader(shader);
            if !gl.get_shader_compile_status(shader) {
                return Err(gl.get_shader_info_log(shader));
            }
            gl.attach_shader(program, shader);
            gl.delete_shader(shader);
        }
        gl.link_program(program);
        if !gl.get_program_link_status(program) {
            return Err(gl.get_program_info_log(program));
        }
        Ok((program, gl.create_vertex_array()?))
    }
}

/// Подключает рендер к GLArea. `center` — центр кнопки Play, обновляется вызывающим перед каждым кадром.
/// `on_fail` вызывается, если OpenGL не поднялся (нет EGL, драйвер без GLSL 3.30/ES 3.0): вызывающий показывает запасной фон.
pub fn attach(area: &GLArea, state: Rc<RefCell<WaveShaderState>>, center: Rc<Cell<(f64, f64)>>, on_fail: impl Fn() + 'static) {
    let gl_state: Rc<RefCell<Option<Gl>>> = Rc::default();

    let s = gl_state.clone();
    area.connect_realize(move |area| {
        area.make_current();
        if let Some(err) = area.error() {
            eprintln!("[wave] OpenGL недоступен: {err}");
            on_fail();
            return;
        }
        let Some(gl) = load_gl() else {
            eprintln!("[wave] не удалось загрузить libEGL");
            on_fail();
            return;
        };
        let es = area.context().is_some_and(|c| c.uses_es());
        match compile(&gl, es) {
            Ok((program, vao)) => *s.borrow_mut() = Some(Gl { gl, program, vao }),
            Err(e) => {
                eprintln!("[wave] шейдер не собрался: {e}");
                on_fail();
            }
        }
    });

    let s = gl_state.clone();
    area.connect_unrealize(move |area| {
        area.make_current();
        if let Some(g) = s.borrow_mut().take() {
            unsafe {
                g.gl.delete_program(g.program);
                g.gl.delete_vertex_array(g.vao);
            }
        }
    });

    area.connect_render(move |area, _| {
        let guard = gl_state.borrow();
        let Some(g) = guard.as_ref() else { return glib::Propagation::Proceed };
        let st = state.borrow();
        let (cx, cy) = center.get();
        let scale = area.scale_factor() as f32;
        unsafe {
            let gl = &g.gl;
            gl.use_program(Some(g.program));
            let loc = |n: &str| gl.get_uniform_location(g.program, n);
            gl.uniform_2_f32(loc("u_size").as_ref(), area.width() as f32, area.height() as f32);
            gl.uniform_1_f32(loc("u_scale").as_ref(), scale);
            gl.uniform_2_f32(loc("u_center").as_ref(), cx as f32, cy as f32);
            gl.uniform_1_f32(loc("u_time").as_ref(), st.time as f32);
            gl.uniform_1_f32(loc("u_bass").as_ref(), st.bass as f32);
            let v3 = |c: [f64; 3]| (c[0] as f32, c[1] as f32, c[2] as f32);
            for (name, c) in [("u_core", st.cur_core), ("u_deep", st.cur_deep), ("u_ridge", st.cur_ridge)] {
                let (r, gg, b) = v3(c);
                gl.uniform_3_f32(loc(name).as_ref(), r, gg, b);
            }
            gl.bind_vertex_array(Some(g.vao));
            gl.draw_arrays(glow::TRIANGLES, 0, 3);
        }
        glib::Propagation::Stop
    });
}
