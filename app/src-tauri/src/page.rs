//! The game's pages (the server's welcome message, help) in a window of their own, as the phone shows them in its
//! web view: the page's scripts and pictures work, but the window is a sandbox.
//!
//! - It has no capability (`capabilities/`), and the app commands go through the access control list
//!   (`build.rs`), so nothing in it can call the launcher's backend or a plugin.
//! - It is served from its own scheme (another origin than the launcher's pages) with a policy that allows inline
//!   scripts and styles and pictures, but no requests, frames, forms or plugins.
//! - It never navigates: links go to the default browser, everything else is refused.

use std::sync::Mutex;
use tauri::http::{header, Request, Response, StatusCode};
use tauri::webview::{NewWindowResponse, PageLoadEvent};
use tauri::{AppHandle, Manager, Runtime, Url, WebviewUrl, WebviewWindowBuilder};

/// Label of the page window.
pub const LABEL: &str = "game-page";
/// The scheme the page is served from.
pub const SCHEME: &str = "bhpage";

/// Content policy of the page: its own inline scripts and styles, pictures from anywhere; no requests, frames,
/// forms, plugins or base URL.
pub const CSP: &str = "default-src 'none'; script-src 'unsafe-inline'; style-src 'unsafe-inline'; \
                       img-src data: https: http:; font-src data:; media-src data:; connect-src 'none'; \
                       frame-src 'none'; object-src 'none'; form-action 'none'; base-uri 'none'";

/// The page being shown.
#[derive(Default)]
pub struct Page(Mutex<String>);

/// The URL of the page (WebView2 and Android reach custom schemes over `http://<scheme>.localhost`).
fn page_url() -> Url {
    let s = if cfg!(windows) { format!("http://{SCHEME}.localhost/") } else { format!("{SCHEME}://localhost/") };
    Url::parse(&s).expect("valid page URL")
}

/// Whether `url` is the page itself (the only place the window may load).
fn is_page(url: &Url) -> bool {
    url.scheme() == SCHEME || (url.scheme() == "http" && url.host_str() == Some(&format!("{SCHEME}.localhost")))
}

/// A link the default browser may open.
fn is_link(url: &Url) -> bool {
    matches!(url.scheme(), "http" | "https" | "mailto") && !is_page(url)
}

/// The page's `<title>`, else the app's name.
fn title(html: &str) -> String {
    let lower = html.to_ascii_lowercase();
    let t = lower
        .find("<title>")
        .and_then(|a| lower[a + 7..].find("</title>").map(|b| html[a + 7..a + 7 + b].trim().to_string()));
    t.filter(|t| !t.is_empty()).unwrap_or_else(|| "Blockheads PC".to_string())
}

/// Serves the page to the page window only.
pub fn serve<R: Runtime>(ctx: tauri::UriSchemeContext<'_, R>, _req: Request<Vec<u8>>) -> Response<Vec<u8>> {
    let mine = ctx.webview_label() == LABEL;
    let body = if mine {
        ctx.app_handle().state::<Page>().0.lock().map(|p| p.clone()).unwrap_or_default()
    } else {
        String::new()
    };
    Response::builder()
        .status(if mine { StatusCode::OK } else { StatusCode::FORBIDDEN })
        .header(header::CONTENT_TYPE, "text/html; charset=utf-8")
        .header(header::CONTENT_SECURITY_POLICY, CSP)
        .header(header::CACHE_CONTROL, "no-store")
        .body(body.into_bytes())
        .expect("valid response")
}

/// Opens a link in the default browser (anything else is dropped).
fn open_link<R: Runtime>(app: &AppHandle<R>, url: &Url) {
    if is_link(url) {
        use tauri_plugin_opener::OpenerExt;
        let _ = app.opener().open_url(url.as_str(), None::<&str>);
    }
}

/// Shows `html` in the page window (a page already open is replaced), above the launcher's window.
pub fn show<R: Runtime>(app: &AppHandle<R>, html: String) -> tauri::Result<()> {
    let name = title(&html);
    if let Ok(mut p) = app.state::<Page>().0.lock() {
        *p = html;
    }
    if let Some(w) = app.get_webview_window(LABEL) {
        w.destroy()?;
    }
    let (nav, popup) = (app.clone(), app.clone());
    let mut b = WebviewWindowBuilder::new(app, LABEL, WebviewUrl::CustomProtocol(page_url()))
        .title(name)
        .inner_size(760.0, 600.0)
        .min_inner_size(360.0, 260.0)
        .center()
        .focused(true)
        .on_navigation(move |url| {
            if is_page(url) {
                return true;
            }
            open_link(&nav, url);
            false
        })
        .on_new_window(move |url, _| {
            open_link(&popup, &url);
            NewWindowResponse::Deny
        })
        .on_page_load(|w, p| {
            if p.event() == PageLoadEvent::Finished {
                let _ = w.set_focus();
            }
        });
    if let Some(main) = app.get_webview_window("main") {
        b = b.parent(&main)?;
    }
    b.build().map(|_| ())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_the_page_loads_and_only_links_leave() {
        let u = |s: &str| Url::parse(s).unwrap();
        assert!(is_page(&page_url()));
        assert!(is_page(&u("bhpage://localhost/x")) && is_page(&u("http://bhpage.localhost/")));
        assert!(!is_page(&u("https://example.com")) && !is_page(&u("tauri://localhost")));
        assert!(is_link(&u("https://example.com/a")) && is_link(&u("http://x.net")) && is_link(&u("mailto:a@b")));
        for s in ["file:///etc/passwd", "tauri://localhost/", "javascript:alert(1)", "http://bhpage.localhost/"] {
            assert!(!is_link(&u(s)), "{s}");
        }
    }

    #[test]
    fn the_title_comes_from_the_page() {
        assert_eq!(title("<html><head><TITLE> Sunny </TITLE></head>"), "Sunny");
        assert_eq!(title("<title></title><p>x"), "Blockheads PC");
        assert_eq!(title("<p>no title"), "Blockheads PC");
        assert_eq!(title("<title>open"), "Blockheads PC");
    }

    #[test]
    fn the_policy_allows_inline_scripts_but_no_requests() {
        assert!(CSP.contains("script-src 'unsafe-inline'") && CSP.contains("connect-src 'none'"));
        assert!(CSP.starts_with("default-src 'none'") && CSP.contains("form-action 'none'"));
    }
}
