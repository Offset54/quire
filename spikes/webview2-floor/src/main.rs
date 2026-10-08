//! M0 task 13 spike: how fast can a bare WebView2 (no Tauri, no wry) show its
//! first frame on this machine? This gives the startup floor of the §5.1
//! stack.
//!
//! Creates a window, a WebView2 environment with its own user data folder (as
//! Tauri does), and a controller. It loads a tiny page that posts back the
//! time of its first frame, writes timings as JSON to `SPIKE_OUT` (default
//! `webview2-floor.json`), and exits.
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

#[cfg(windows)]
mod imp {
    use std::sync::mpsc;
    use std::time::{SystemTime, UNIX_EPOCH};

    use webview2_com::Microsoft::Web::WebView2::Win32::{
        CreateCoreWebView2EnvironmentWithOptions, ICoreWebView2Controller, ICoreWebView2Environment,
    };
    use webview2_com::{
        CoTaskMemPWSTR, CreateCoreWebView2ControllerCompletedHandler,
        CreateCoreWebView2EnvironmentCompletedHandler, WebMessageReceivedEventHandler,
    };
    use windows::Win32::Foundation::{E_POINTER, FILETIME, HWND, LPARAM, LRESULT, RECT, WPARAM};
    use windows::Win32::System::Com::{COINIT_APARTMENTTHREADED, CoInitializeEx};
    use windows::Win32::System::LibraryLoader::GetModuleHandleW;
    use windows::Win32::System::Threading::{GetCurrentProcess, GetProcessTimes};
    use windows::Win32::UI::WindowsAndMessaging::{
        CW_USEDEFAULT, CreateWindowExW, DefWindowProcW, DispatchMessageW, GetClientRect,
        GetMessageW, MSG, PostQuitMessage, RegisterClassW, TranslateMessage, WINDOW_EX_STYLE,
        WNDCLASSW, WS_OVERLAPPEDWINDOW, WS_VISIBLE,
    };
    use windows_core::{HSTRING, PCWSTR, PWSTR, w};

    fn now_ms() -> f64 {
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_secs_f64()
            * 1e3
    }

    fn process_created_ms() -> f64 {
        let mut t = [FILETIME::default(); 4];
        let [c, e, k, u] = &mut t;
        // SAFETY: valid out-parameters for the current process.
        unsafe { GetProcessTimes(GetCurrentProcess(), c, e, k, u) }.unwrap();
        let ticks = (u64::from(t[0].dwHighDateTime) << 32) | u64::from(t[0].dwLowDateTime);
        ticks as f64 / 10_000.0 - 11_644_473_600_000.0
    }

    extern "system" fn wndproc(hwnd: HWND, msg: u32, wp: WPARAM, lp: LPARAM) -> LRESULT {
        // SAFETY: default handling for every message.
        unsafe { DefWindowProcW(hwnd, msg, wp, lp) }
    }

    const PAGE: &str = r#"<!doctype html><html><body style="margin:0;background:#f7f7f5"><p style="font:14px system-ui;color:#6b6f85;text-align:center;margin-top:40vh">No document open</p><script>requestAnimationFrame(()=>setTimeout(()=>chrome.webview.postMessage(String(performance.timeOrigin+performance.now())),0));</script></body></html>"#;

    pub fn main() -> windows_core::Result<()> {
        let created = process_created_ms();
        let main_at = now_ms();
        // SAFETY: plain Win32/COM setup on this thread; every handle used
        // below is created here and lives until the process exits.
        unsafe {
            CoInitializeEx(None, COINIT_APARTMENTTHREADED).ok()?;
            let instance = GetModuleHandleW(None)?;
            let class = WNDCLASSW {
                lpfnWndProc: Some(wndproc),
                hInstance: instance.into(),
                lpszClassName: w!("webview2_floor"),
                ..Default::default()
            };
            RegisterClassW(&class);
            let hwnd = CreateWindowExW(
                WINDOW_EX_STYLE(0),
                w!("webview2_floor"),
                w!("webview2 floor"),
                WS_OVERLAPPEDWINDOW | WS_VISIBLE,
                CW_USEDEFAULT,
                CW_USEDEFAULT,
                1280,
                720,
                None,
                None,
                Some(instance.into()),
                None,
            )?;
            let window_at = now_ms();

            let data = std::env::var("SPIKE_DATA").unwrap_or_else(|_| {
                std::env::temp_dir()
                    .join("webview2-floor-data")
                    .to_string_lossy()
                    .into_owned()
            });
            let data = HSTRING::from(data);
            let (tx, rx) = mpsc::channel();
            CreateCoreWebView2EnvironmentCompletedHandler::wait_for_async_operation(
                Box::new(move |handler| {
                    CreateCoreWebView2EnvironmentWithOptions(PCWSTR::null(), &data, None, &handler)
                        .map_err(webview2_com::Error::WindowsError)
                }),
                Box::new(move |code, env: Option<ICoreWebView2Environment>| {
                    code?;
                    tx.send(env.ok_or_else(|| windows_core::Error::from(E_POINTER)))
                        .unwrap();
                    Ok(())
                }),
            )
            .unwrap();
            let env = rx.recv().unwrap()?;
            let env_at = now_ms();

            let (tx, rx) = mpsc::channel();
            let env2 = env.clone();
            CreateCoreWebView2ControllerCompletedHandler::wait_for_async_operation(
                Box::new(move |handler| {
                    env2.CreateCoreWebView2Controller(hwnd, &handler)
                        .map_err(webview2_com::Error::WindowsError)
                }),
                Box::new(move |code, c: Option<ICoreWebView2Controller>| {
                    code?;
                    tx.send(c.ok_or_else(|| windows_core::Error::from(E_POINTER)))
                        .unwrap();
                    Ok(())
                }),
            )
            .unwrap();
            let controller = rx.recv().unwrap()?;
            let controller_at = now_ms();

            let mut rect = RECT::default();
            GetClientRect(hwnd, &mut rect)?;
            controller.SetBounds(rect)?;
            controller.SetIsVisible(true)?;
            let webview = controller.CoreWebView2()?;

            let mut token = 0;
            webview.add_WebMessageReceived(
                &WebMessageReceivedEventHandler::create(Box::new(move |_, args| {
                    if let Some(args) = args {
                        let mut msg = PWSTR::null();
                        args.TryGetWebMessageAsString(&mut msg)?;
                        let first_frame: f64 =
                            CoTaskMemPWSTR::from(msg).to_string().parse().unwrap_or(0.0);
                        let out = serde_json::json!({
                            "processCreatedEpochMs": created,
                            "mainMs": main_at - created,
                            "windowMs": window_at - created,
                            "environmentMs": env_at - created,
                            "controllerMs": controller_at - created,
                            "firstFrameMs": first_frame - created,
                        });
                        let path = std::env::var("SPIKE_OUT")
                            .unwrap_or_else(|_| "webview2-floor.json".into());
                        std::fs::write(path, out.to_string()).unwrap();
                        // SPIKE_STAY keeps this instance (and its WebView2
                        // browser process) alive, to measure a warm attach.
                        if std::env::var_os("SPIKE_STAY").is_none() {
                            PostQuitMessage(0);
                        }
                    }
                    Ok(())
                })),
                &mut token,
            )?;
            webview.NavigateToString(&HSTRING::from(PAGE))?;

            let mut msg = MSG::default();
            while GetMessageW(&mut msg, None, 0, 0).into() {
                let _ = TranslateMessage(&msg);
                DispatchMessageW(&msg);
            }
            drop(controller);
        }
        Ok(())
    }
}

fn main() {
    #[cfg(windows)]
    imp::main().expect("webview2 floor spike failed");
    #[cfg(not(windows))]
    eprintln!("webview2-floor is Windows-only");
}
