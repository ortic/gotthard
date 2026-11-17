//! Gotthard – file sync and management around a master copy, without a cloud.

// Release builds are GUI programs on Windows, without a console window;
// debug builds keep it for development.
#![cfg_attr(all(windows, not(debug_assertions)), windows_subsystem = "windows")]

mod app;
#[cfg(test)]
mod screenshots;
mod theme;
mod views;
mod widgets;

use std::fs::{self, File};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

use gotthard_config::Paths;
use tracing::{error, info, warn};
use tracing_subscriber::{EnvFilter, fmt, prelude::*};

fn main() -> anyhow::Result<()> {
    attach_parent_console();
    let paths = match Paths::detect() {
        Ok(paths) => Some(paths),
        Err(e) => {
            report_error(&format!(
                "Gotthard could not find its settings folder ({e:#}). It starts without saved settings."
            ));
            None
        }
    };
    init_logging(paths.as_ref());
    install_panic_hook(paths.as_ref());
    info!(version = env!("CARGO_PKG_VERSION"), "starting Gotthard");

    // Set once the app exists: a later error isn't the renderer's.
    let started = Arc::new(AtomicBool::new(false));
    let result = run(eframe::Renderer::Wgpu, paths.clone(), &started).or_else(|e| {
        if started.load(Ordering::SeqCst) {
            return Err(e);
        }
        warn!("wgpu renderer failed ({e}), falling back to OpenGL");
        run(eframe::Renderer::Glow, paths.clone(), &started)
    });
    if let Err(e) = &result {
        report_error(&if started.load(Ordering::SeqCst) {
            format!("Gotthard lost its window ({e}).")
        } else {
            format!(
                "Gotthard could not open its window: neither Vulkan/DirectX nor OpenGL works here ({e})."
            )
        });
    }
    result.map_err(|e| anyhow::anyhow!("{e}"))
}

/// Reports a problem where it's seen: there may be no console (a GUI
/// program on Windows), so also in a message box.
fn report_error(text: &str) {
    error!("{text}");
    eprintln!("gotthard: {text}");
    let _ = rfd::MessageDialog::new()
        .set_level(rfd::MessageLevel::Error)
        .set_title("Gotthard")
        .set_description(text)
        .set_buttons(rfd::MessageButtons::Ok)
        .show();
}

/// Started from a terminal, a Windows GUI program has no console of its own;
/// attaching to the terminal's shows the log output there.
fn attach_parent_console() {
    #[cfg(all(windows, not(debug_assertions)))]
    {
        unsafe extern "system" {
            fn AttachConsole(process_id: u32) -> i32;
        }
        const ATTACH_PARENT_PROCESS: u32 = u32::MAX;
        // Fails when there's no parent console, e.g. started from Explorer.
        unsafe {
            AttachConsole(ATTACH_PARENT_PROCESS);
        }
    }
}

/// Runs the app until its window closes; sets `started` once the renderer
/// works and the app exists.
fn run(
    renderer: eframe::Renderer,
    paths: Option<Paths>,
    started: &Arc<AtomicBool>,
) -> eframe::Result {
    let started = started.clone();
    let options = eframe::NativeOptions {
        renderer,
        viewport: egui::ViewportBuilder::default()
            .with_title("Gotthard")
            .with_app_id("gotthard")
            .with_inner_size([1280.0, 800.0])
            .with_min_inner_size([800.0, 500.0])
            .with_icon(window_icon()),
        event_loop_builder: x11_on_wslg(),
        ..Default::default()
    };
    eframe::run_native(
        "gotthard",
        options,
        Box::new(move |cc| {
            started.store(true, Ordering::SeqCst);
            Ok(Box::new(app::App::new(cc, paths)))
        }),
    )
}

/// Under WSLg the window opens through X11 (Xwayland): WSLg's compositor
/// crashes when a Wayland client grows its cursor pool (microsoft/wslg#1386).
/// `GOTTHARD_WAYLAND=1` keeps Wayland.
#[cfg(target_os = "linux")]
fn x11_on_wslg() -> Option<eframe::EventLoopBuilderHook> {
    let kernel = fs::read_to_string("/proc/sys/kernel/osrelease").unwrap_or_default();
    let set = |name: &str| std::env::var_os(name).is_some_and(|v| !v.is_empty());
    if !(kernel.to_ascii_lowercase().contains("microsoft")
        && set("WAYLAND_DISPLAY")
        && set("DISPLAY")
        && !set("GOTTHARD_WAYLAND"))
    {
        return None;
    }
    Some(Box::new(|builder| {
        use winit::platform::x11::EventLoopBuilderExtX11;
        builder.with_x11();
    }))
}

#[cfg(not(target_os = "linux"))]
fn x11_on_wslg() -> Option<eframe::EventLoopBuilderHook> {
    None
}

/// The application icon, embedded at build time.
fn window_icon() -> egui::IconData {
    eframe::icon_data::from_png_bytes(include_bytes!("../../../assets/icon/gotthard-256.png"))
        .expect("embedded icon is a valid PNG")
}

/// Logs to stderr and to `<data dir>/logs/gotthard.log`. `RUST_LOG` overrides the filter.
fn init_logging(paths: Option<&Paths>) {
    let filter = || {
        EnvFilter::try_from_default_env()
            .unwrap_or_else(|_| EnvFilter::new("info,wgpu=warn,naga=warn,eframe=warn"))
    };
    let file = paths.and_then(|p| {
        let dir = p.log_dir();
        fs::create_dir_all(&dir).ok()?;
        File::create(dir.join("gotthard.log")).ok()
    });
    let file_layer = file.map(|f| {
        fmt::layer()
            .with_ansi(false)
            .with_writer(Mutex::new(f))
            .with_filter(filter())
    });
    tracing_subscriber::registry()
        .with(
            fmt::layer()
                .with_writer(std::io::stderr)
                .with_filter(filter()),
        )
        .with(file_layer)
        .init();
}

/// Logs panics and writes a crash report next to the log file.
fn install_panic_hook(paths: Option<&Paths>) {
    let crash_dir = paths.map(Paths::log_dir);
    let default_hook = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |panic| {
        let backtrace = std::backtrace::Backtrace::force_capture();
        error!("panic: {panic}\n{backtrace}");
        if let Some(dir) = &crash_dir {
            let stamp = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map_or(0, |d| d.as_secs());
            let _ = fs::write(
                dir.join(format!("crash-{stamp}.txt")),
                format!("{panic}\n\n{backtrace}"),
            );
        }
        default_hook(panic);
    }));
}
