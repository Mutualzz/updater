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

// The splash HTML is embedded at compile time so we ship a single binary
const SPLASH_HTML: &str = include_str!("splash.html");

// Logo PNG embedded as base64 so it works inside the WebView's data: URL context
const LOGO_PNG: &[u8] = include_bytes!("../resources/icon.png");

/// Custom event so the async runtime can poke the tao event loop
#[derive(Debug)]
enum UserEvent {
    Cmd(SplashCmd),
}

pub fn run(rx: Receiver<SplashCmd>) {
    let event_loop: EventLoop<UserEvent> = EventLoopBuilder::with_user_event().build();
    let proxy = event_loop.create_proxy();

    // Forward SplashCmds from the async thread → tao event loop
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
        .with_decorations(false)         // frameless
        .with_transparent(false)
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

    // Suppress macOS dock icon — updater is a utility process
    #[cfg(target_os = "macos")]
    set_activation_policy_accessory();

    // Encode logo as base64 data URL
    use base64::Engine;
    let logo_b64 = base64::engine::general_purpose::STANDARD.encode(LOGO_PNG);
    let logo_data_url = format!("data:image/png;base64,{}", logo_b64);

    // Inject the logo data URL into the HTML before loading
    let html = SPLASH_HTML.replace("__LOGO_DATA_URL__", &logo_data_url);
    let html_data_url = format!(
        "data:text/html;charset=utf-8,{}",
        urlencoding_encode(&html)
    );

    let webview = WebViewBuilder::new()
        .with_url(&html_data_url)
        .with_transparent(false)
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

/// Simple percent-encoding for the data URL
fn urlencoding_encode(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for b in s.bytes() {
        match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9'
            | b'-' | b'_' | b'.' | b'!' | b'~' | b'*'
            | b'\'' | b'(' | b')' | b'/' | b'<' | b'>'
            | b'=' | b':' | b',' | b';' | b'@' | b'#'
            | b'+' | b'?' | b'&' | b' ' | b'\n' | b'\t' => {
                out.push(b as char);
            }
            _ => {
                out.push('%');
                out.push_str(&format!("{:02X}", b));
            }
        }
    }
    out
}

#[cfg(target_os = "macos")]
fn set_activation_policy_accessory() {
    use objc::{msg_send, sel, sel_impl, class};
    unsafe {
        let app: *mut objc::runtime::Object =
            msg_send![class!(NSApplication), sharedApplication];
        let _: () = msg_send![app, setActivationPolicy: 1i64]; // NSApplicationActivationPolicyAccessory
    }
}