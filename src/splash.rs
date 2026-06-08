use std::sync::mpsc::Receiver;
use tao::{
    dpi::LogicalSize,
    event::{Event, WindowEvent},
    event_loop::{ControlFlow, EventLoop, EventLoopBuilder},
    window::WindowBuilder,
};
use wry::WebViewBuilder;
use log::{error, info};

use crate::SplashCmd;

const SPLASH_HTML: &str = include_str!("splash.html");
const LOGO_PNG: &[u8] = include_bytes!("../resources/logo.png");

#[derive(Debug)]
enum UserEvent {
    Cmd(SplashCmd),
}

pub fn run(rx: Receiver<SplashCmd>) {
    let event_loop: EventLoop<UserEvent> = EventLoopBuilder::with_user_event().build();
    let proxy = event_loop.create_proxy();

    std::thread::spawn(move || {
        while let Ok(cmd) = rx.recv() {
            let should_close = matches!(cmd, SplashCmd::Close);
            let _ = proxy.send_event(UserEvent::Cmd(cmd));
            if should_close {
                break;
            }
        }
    });

    let window = WindowBuilder::new()
        .with_title("Mutualzz")
        .with_inner_size(LogicalSize::new(300u32, 340u32))
        .with_resizable(false)
        .with_decorations(false)
        .with_transparent(true)
        .with_always_on_top(true)
        .build(&event_loop)
        .expect("Failed to create splash window");

    // Center the window
    if let Some(monitor) = window.current_monitor() {
        let screen = monitor.size();
        let win = window.outer_size();
        let x = (screen.width as i32 - win.width as i32) / 2;
        let y = (screen.height as i32 - win.height as i32) / 2;
        window.set_outer_position(tao::dpi::PhysicalPosition::new(x, y));
    }

    #[cfg(target_os = "macos")]
    {
        set_activation_policy_accessory();

        // Allow dragging the window by clicking anywhere on it
        use tao::platform::macos::WindowExtMacOS;
        window.set_movable_by_window_background(true);

        // Round the OS-level window corners
        set_window_corner_radius(&window, 16.0);
    }

    use base64::Engine;
    let logo_b64 = base64::engine::general_purpose::STANDARD.encode(LOGO_PNG);
    let logo_data_url = format!("data:image/png;base64,{}", logo_b64);
    let html = SPLASH_HTML.replace("__LOGO_DATA_URL__", &logo_data_url);

    let webview = WebViewBuilder::new()
        .with_html(html)
        .with_transparent(true)
        // Set native background color to prevent white flash before HTML renders
        .with_background_color((36, 25, 39, 255))  // #241927
        // Set to true to right-click inspect during development
        .with_devtools(false)
        .build(&window)
        .expect("Failed to create WebView");

    info!("Splash window open");

    event_loop.run(move |event, _, control_flow| {
        *control_flow = ControlFlow::Wait;

        match event {
            Event::UserEvent(UserEvent::Cmd(cmd)) => {
                handle_cmd(&webview, cmd, control_flow);
            }
            Event::WindowEvent {
                event: WindowEvent::CloseRequested,
                ..
            } => {
                *control_flow = ControlFlow::Exit;
            }
            _ => {}
        }
    });
}

fn handle_cmd(
    webview: &wry::WebView,
    cmd: SplashCmd,
    control_flow: &mut ControlFlow,
) {
    match cmd {
        SplashCmd::SetStatus(text) => {
            let js = format!("window.setStatus({})", serde_json::to_string(&text).unwrap());
            if let Err(e) = webview.evaluate_script(&js) {
                error!("JS eval error: {}", e);
            }
        }
        SplashCmd::SetProgress(pct) => {
            let js = format!("window.setProgress({})", pct);
            if let Err(e) = webview.evaluate_script(&js) {
                error!("JS eval error: {}", e);
            }
        }
        SplashCmd::HideProgress => {
            if let Err(e) = webview.evaluate_script("window.hideProgress()") {
                error!("JS eval error: {}", e);
            }
        }
        SplashCmd::Close => {
            info!("Closing splash window");
            *control_flow = ControlFlow::Exit;
        }
    }
}

#[cfg(target_os = "macos")]
fn set_activation_policy_accessory() {
    use objc::{msg_send, sel, sel_impl, class};
    unsafe {
        let app: *mut objc::runtime::Object =
            msg_send![class!(NSApplication), sharedApplication];
        let _: () = msg_send![app, setActivationPolicy: 1i64];
    }
}

#[cfg(target_os = "macos")]
fn set_window_corner_radius(window: &tao::window::Window, radius: f64) {
    use objc::{msg_send, sel, sel_impl};
    use tao::platform::macos::WindowExtMacOS;
    unsafe {
        let ns_window = window.ns_window() as *mut objc::runtime::Object;

        // Round the window itself
        let _: () = msg_send![ns_window, setCornerMask: 9u64]; // NSWindowMaskRounded

        // Round the content view layer
        let content_view: *mut objc::runtime::Object =
            msg_send![ns_window, contentView];
        let _: () = msg_send![content_view, setWantsLayer: true];
        let layer: *mut objc::runtime::Object =
            msg_send![content_view, layer];
        let _: () = msg_send![layer, setCornerRadius: radius];
        let _: () = msg_send![layer, setMasksToBounds: true];
    }
}