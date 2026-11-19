//! Auro Inspector: a small native HTTP/API client.
//!
//! The request form, HTTP work, JSON rendering and persistence live in this host view. The
//! `data` crate remains the hot boundary: editing its title/accent reloads the header in the same
//! process, while editing this file is a host change and restarts the app.

use std::fs;
use std::io::Read as _;
use std::path::PathBuf;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use auro::gpui::{App as GpuiApp, AppContext, Entity, Global, ScrollHandle};
use auro::prelude::*;
use auro_inspector_types::{HotData, ViewData};
use chrono::Local;
use gpui_component::{
    ActiveTheme, Root, Theme as KitTheme, ThemeMode, button::Button as KitButton, h_flex,
    input::Input, input::InputState, v_flex,
};
use serde::{Deserialize, Serialize};
use serde_json::Value;

const MAX_RESPONSE_BYTES: u64 = 2 * 1024 * 1024;
const MAX_HISTORY: usize = 100;

#[derive(Clone, Debug, Serialize, Deserialize)]
struct HistoryEntry {
    method: String,
    url: String,
    status: u16,
    duration_ms: u64,
    at: String,
}

#[derive(Clone, Debug)]
struct ResponseView {
    status: u16,
    reason: String,
    headers: String,
    body: String,
    pretty: String,
    duration_ms: u64,
}

#[derive(Clone, Debug, Default)]
struct InspectorModel {
    history: Vec<HistoryEntry>,
    response: Option<ResponseView>,
    loading: bool,
    message: Option<String>,
}

struct ModelGlobal(Entity<InspectorModel>);

impl Global for ModelGlobal {}

fn data_dir() -> PathBuf {
    let base = std::env::var_os("XDG_DATA_HOME")
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|home| PathBuf::from(home).join(".local/share")))
        .unwrap_or_else(|| PathBuf::from("."));
    base.join("auro-inspector")
}

fn history_path() -> PathBuf {
    data_dir().join("history.json")
}

fn load_history() -> Vec<HistoryEntry> {
    let path = history_path();
    let Ok(text) = fs::read_to_string(&path) else {
        return Vec::new();
    };
    serde_json::from_str(&text).unwrap_or_else(|error| {
        eprintln!(
            "auro-inspector: ignoring unreadable history {}: {error}",
            path.display()
        );
        Vec::new()
    })
}

fn save_history(history: &[HistoryEntry]) {
    let path = history_path();
    let result = path
        .parent()
        .map_or(Ok(()), fs::create_dir_all)
        .and_then(|()| serde_json::to_vec_pretty(history).map_err(std::io::Error::other))
        .and_then(|bytes| fs::write(&path, bytes));
    if let Err(error) = result {
        eprintln!("auro-inspector: history write failed: {error}");
    }
}

fn export_body(body: &str) -> Result<PathBuf, String> {
    let dir = data_dir().join("exports");
    fs::create_dir_all(&dir).map_err(|error| format!("create {}: {error}", dir.display()))?;
    let stamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_millis())
        .unwrap_or(0);
    let path = dir.join(format!("response-{stamp}.json"));
    fs::write(&path, body).map_err(|error| format!("write {}: {error}", path.display()))?;
    Ok(path)
}

fn perform_request(method: &str, url: &str, body: &str) -> Result<ResponseView, String> {
    let started = Instant::now();
    let config = ureq::Agent::config_builder()
        .http_status_as_error(false)
        .timeout_connect(Some(Duration::from_secs(10)))
        .timeout_recv_response(Some(Duration::from_secs(30)))
        .timeout_recv_body(Some(Duration::from_secs(30)))
        .build();
    let agent = ureq::Agent::new_with_config(config);
    let request = match method {
        "GET" => agent.get(url).header("Accept", "application/json").call(),
        "POST" => agent
            .post(url)
            .header("Accept", "application/json")
            .header("Content-Type", "application/json")
            .send(body),
        "PUT" => agent
            .put(url)
            .header("Accept", "application/json")
            .header("Content-Type", "application/json")
            .send(body),
        "PATCH" => agent
            .patch(url)
            .header("Accept", "application/json")
            .header("Content-Type", "application/json")
            .send(body),
        "DELETE" => agent
            .delete(url)
            .header("Accept", "application/json")
            .call(),
        other => return Err(format!("unsupported HTTP method `{other}`")),
    };
    let mut response = request.map_err(|error| error.to_string())?;
    let status = response.status().as_u16();
    let reason = response
        .status()
        .canonical_reason()
        .unwrap_or("")
        .to_owned();
    let headers = response
        .headers()
        .iter()
        .map(|(name, value)| format!("{}: {}", name, value.to_str().unwrap_or("<binary header>")))
        .collect::<Vec<_>>()
        .join("\n");
    let mut body_text = String::new();
    response
        .body_mut()
        .as_reader()
        .take(MAX_RESPONSE_BYTES)
        .read_to_string(&mut body_text)
        .map_err(|error| error.to_string())?;
    let pretty = serde_json::from_str::<Value>(&body_text)
        .ok()
        .and_then(|value| serde_json::to_string_pretty(&value).ok())
        .unwrap_or_else(|| body_text.clone());
    Ok(ResponseView {
        status,
        reason,
        headers,
        body: body_text,
        pretty,
        duration_ms: started.elapsed().as_millis() as u64,
    })
}

#[cfg(feature = "hot-reload")]
fn current_frame(data: fn() -> HotData) -> Result<HotData, String> {
    std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| auro::hot::hot_frame(data)))
        .map_err(|_| "the patched boundary panicked".to_owned())
}

#[cfg(not(feature = "hot-reload"))]
fn current_frame(data: fn() -> HotData) -> Result<HotData, String> {
    Ok(data())
}

pub struct InspectorView {
    model: Entity<InspectorModel>,
    data: fn() -> HotData,
    method: Entity<InputState>,
    url: Entity<InputState>,
    body: Entity<InputState>,
    response_scroll: ScrollHandle,
    last_data: Option<ViewData>,
}

impl InspectorView {
    fn new(
        model: Entity<InspectorModel>,
        data: fn() -> HotData,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let method = cx.new(|cx| InputState::new(window, cx).placeholder("GET"));
        let url = cx.new(|cx| {
            InputState::new(window, cx)
                .placeholder("https://api.github.com/repos/zed-industries/zed")
        });
        let body = cx.new(|cx| {
            InputState::new(window, cx)
                .placeholder("{\"key\": \"value\"}")
                .multi_line(true)
                .rows(5)
        });
        cx.observe(&model, |_, _, cx| cx.notify()).detach();
        InspectorView {
            model,
            data,
            method,
            url,
            body,
            response_scroll: ScrollHandle::default(),
            last_data: None,
        }
    }

    fn load_entry(&mut self, entry: &HistoryEntry, window: &mut Window, cx: &mut Context<Self>) {
        self.method.update(cx, |state, cx| {
            state.set_value(entry.method.clone(), window, cx);
        });
        self.url.update(cx, |state, cx| {
            state.set_value(entry.url.clone(), window, cx);
        });
        cx.notify();
    }

    fn clear_history(&mut self, cx: &mut Context<Self>) {
        self.model.update(cx, |model, cx| {
            model.history.clear();
            cx.notify();
        });
        cx.spawn(async move |_this, _cx| {
            let _ = smol::unblock(|| save_history(&[])).await;
        })
        .detach();
    }

    fn send(&mut self, cx: &mut Context<Self>) {
        if self.model.read(cx).loading {
            return;
        }
        let method = self.method.read(cx).value().trim().to_uppercase();
        let method = if method.is_empty() {
            "GET".to_owned()
        } else {
            method
        };
        let url = self.url.read(cx).value().trim().to_owned();
        if url.is_empty() {
            self.model.update(cx, |model, cx| {
                model.message = Some("enter a URL first".to_owned());
                cx.notify();
            });
            return;
        }
        let body = self.body.read(cx).value().to_owned();
        let history = self.model.read(cx).history.clone();
        self.model.update(cx, |model, cx| {
            model.loading = true;
            model.message = None;
            cx.notify();
        });

        let request_method = method.clone();
        let request_url = url.clone();
        cx.spawn(async move |this, cx| {
            let result = smol::unblock(move || {
                let response = perform_request(&request_method, &request_url, &body)?;
                let entry = HistoryEntry {
                    method: request_method.clone(),
                    url: request_url.clone(),
                    status: response.status,
                    duration_ms: response.duration_ms,
                    at: Local::now().format("%H:%M:%S").to_string(),
                };
                let mut history = history;
                history.insert(0, entry);
                history.truncate(MAX_HISTORY);
                save_history(&history);
                Ok::<_, String>((response, history))
            })
            .await;
            this.update(cx, |this, cx| {
                this.model.update(cx, |model, cx| {
                    model.loading = false;
                    match result {
                        Ok((response, history)) => {
                            model.history = history;
                            model.response = Some(response);
                            model.message = None;
                        }
                        Err(error) => {
                            model.message = Some(error);
                        }
                    }
                    cx.notify();
                });
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    fn export(&mut self, cx: &mut Context<Self>) {
        let Some(response) = self.model.read(cx).response.clone() else {
            self.model.update(cx, |model, cx| {
                model.message = Some("no response to export".to_owned());
                cx.notify();
            });
            return;
        };
        cx.spawn(async move |this, cx| {
            let result = smol::unblock(move || export_body(&response.body)).await;
            this.update(cx, |this, cx| {
                this.model.update(cx, |model, cx| {
                    model.message = Some(match result {
                        Ok(path) => format!("exported to {}", path.display()),
                        Err(error) => format!("export failed: {error}"),
                    });
                    cx.notify();
                });
                cx.notify();
            })
            .ok();
        })
        .detach();
    }
}

impl Render for InspectorView {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme().clone();
        let frame = current_frame(self.data)
            .and_then(|frame| frame.decode().map_err(|error| error.to_owned()));
        let data = match frame {
            Ok(data) => {
                #[cfg(feature = "hot-reload")]
                auro::hot::rendered();
                data
            }
            Err(error) => {
                eprintln!("auro-inspector: hot data rejected: {error}; previous values stay");
                self.last_data.clone().unwrap_or_default()
            }
        };
        self.last_data = Some(data.clone());

        let (history, response, loading, message) = {
            let model = self.model.read(cx);
            (
                model.history.clone(),
                model.response.clone(),
                model.loading,
                model.message.clone(),
            )
        };
        let history_rows = history
            .iter()
            .enumerate()
            .map(|(index, entry)| {
                let entry = entry.clone();
                h_flex()
                    .id(("history", index))
                    .gap_2()
                    .p_2()
                    .rounded_md()
                    .cursor_pointer()
                    .text_size(px(12.0))
                    .hover(|style| style.bg(theme.muted))
                    .child(
                        div()
                            .w_10()
                            .text_color(theme.muted_foreground)
                            .child(entry.method.clone()),
                    )
                    .child(
                        div()
                            .w_10()
                            .text_color(if entry.status >= 400 {
                                theme.red
                            } else {
                                theme.green
                            })
                            .child(entry.status.to_string()),
                    )
                    .child(div().flex_1().child(entry.url.clone()))
                    .on_click(cx.listener(move |this, _, window, cx| {
                        this.load_entry(&entry, window, cx);
                    }))
            })
            .collect::<Vec<_>>();

        let status_line = response.as_ref().map(|response| {
            let color = match response.status {
                200..=299 => theme.green,
                300..=399 => theme.yellow,
                _ => theme.red,
            };
            h_flex()
                .gap_3()
                .items_center()
                .child(
                    div()
                        .text_size(px(16.0))
                        .text_color(color)
                        .child(format!("HTTP {} {}", response.status, response.reason)),
                )
                .child(
                    div()
                        .text_size(px(12.0))
                        .text_color(theme.muted_foreground)
                        .child(format!("{} ms", response.duration_ms)),
                )
        });

        let response_headers = response.as_ref().map(|response| {
            div()
                .id("response-headers")
                .max_h(px(96.0))
                .overflow_y_scroll()
                .font_family("monospace")
                .text_size(px(11.0))
                .text_color(theme.muted_foreground)
                .child(response.headers.clone())
        });

        let response_body = response.as_ref().map(|response| {
            div()
                .id("response-scroll")
                .flex_1()
                .overflow_y_scroll()
                .track_scroll(&self.response_scroll)
                .p_3()
                .rounded_md()
                .bg(theme.muted)
                .font_family("monospace")
                .text_size(px(12.0))
                .child(response.pretty.clone())
        });

        v_flex()
            .size_full()
            .bg(theme.background)
            .text_color(theme.foreground)
            .child(
                h_flex()
                    .p_3()
                    .gap_3()
                    .items_center()
                    .bg(rgb(data.accent))
                    .text_color(rgb(0xf8fafc))
                    .child(div().text_size(px(20.0)).child(data.title.clone()))
                    .child(
                        div()
                            .text_size(px(12.0))
                            .child("A native HTTP client built with Auro"),
                    )
                    .child(div().flex_1())
                    .child(
                        KitButton::new("history-window")
                            .label("History window")
                            .on_click(cx.listener(|_, _, _, cx| {
                                if let Err(error) = windows::open("history", cx) {
                                    eprintln!("auro-inspector: history window: {error}");
                                }
                            })),
                    ),
            )
            .child(
                v_flex()
                    .p_3()
                    .gap_3()
                    .border_b_1()
                    .border_color(theme.border)
                    .child(
                        h_flex()
                            .gap_2()
                            .items_center()
                            .child(Input::new(&self.method).w(px(112.0)))
                            .child(Input::new(&self.url).flex_1())
                            .child(
                                KitButton::new("send")
                                    .label(if loading { "Sending..." } else { "Send" })
                                    .on_click(cx.listener(|this, _, _, cx| this.send(cx))),
                            )
                            .child(
                                KitButton::new("export")
                                    .label("Export")
                                    .on_click(cx.listener(|this, _, _, cx| this.export(cx))),
                            ),
                    )
                    .child(
                        h_flex()
                            .gap_2()
                            .items_start()
                            .child(
                                div()
                                    .w(px(112.0))
                                    .text_size(px(12.0))
                                    .text_color(theme.muted_foreground)
                                    .child("Body (POST/PUT/PATCH)"),
                            )
                            .child(Input::new(&self.body).flex_1()),
                    )
                    .child(
                        div()
                            .text_size(px(12.0))
                            .text_color(theme.muted_foreground)
                            .child(message.clone().unwrap_or_else(|| {
                                "GET is the default; click a history entry to load it.".to_owned()
                            })),
                    ),
            )
            .child(
                h_flex()
                    .flex_1()
                    .min_h(px(0.0))
                    .child(
                        v_flex()
                            .flex_1()
                            .min_w(px(0.0))
                            .p_3()
                            .gap_2()
                            .children(status_line)
                            .children(response_headers)
                            .children(response_body)
                            .when(response.is_none() && !loading, |this| {
                                this.child(
                                    div()
                                        .text_color(theme.muted_foreground)
                                        .child("Send a request to see the response."),
                                )
                            }),
                    )
                    .child(
                        v_flex()
                            .w(px(340.0))
                            .h_full()
                            .border_l_1()
                            .border_color(theme.border)
                            .child(
                                h_flex()
                                    .p_3()
                                    .gap_2()
                                    .items_center()
                                    .child(div().text_size(px(16.0)).child("History"))
                                    .child(div().flex_1())
                                    .child(
                                        KitButton::new("clear-history").label("Clear").on_click(
                                            cx.listener(|this, _, _, cx| {
                                                this.clear_history(cx);
                                            }),
                                        ),
                                    ),
                            )
                            .child(
                                div()
                                    .id("history-scroll")
                                    .flex_1()
                                    .overflow_y_scroll()
                                    .px_2()
                                    .children(history_rows),
                            ),
                    ),
            )
    }
}

pub struct HistoryView {
    model: Entity<InspectorModel>,
}

impl HistoryView {
    fn new(model: Entity<InspectorModel>, cx: &mut Context<Self>) -> Self {
        cx.observe(&model, |_, _, cx| cx.notify()).detach();
        HistoryView { model }
    }
}

impl Render for HistoryView {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme().clone();
        let history = self.model.read(cx).history.clone();
        let rows = history
            .iter()
            .enumerate()
            .map(|(index, entry)| {
                v_flex()
                    .p_2()
                    .gap_1()
                    .border_b_1()
                    .border_color(theme.border)
                    .text_size(px(12.0))
                    .child(
                        h_flex()
                            .gap_2()
                            .child(div().text_color(theme.muted_foreground).child(format!(
                                "#{} {}",
                                index + 1,
                                entry.at
                            )))
                            .child(div().child(entry.method.clone()))
                            .child(div().child(entry.status.to_string()))
                            .child(div().child(format!("{} ms", entry.duration_ms))),
                    )
                    .child(
                        div()
                            .text_color(theme.muted_foreground)
                            .child(entry.url.clone()),
                    )
            })
            .collect::<Vec<_>>();

        v_flex()
            .size_full()
            .bg(theme.background)
            .text_color(theme.foreground)
            .child(
                h_flex()
                    .p_3()
                    .gap_2()
                    .items_center()
                    .border_b_1()
                    .border_color(theme.border)
                    .child(div().text_size(px(18.0)).child("Request history"))
                    .child(div().flex_1())
                    .child(
                        KitButton::new("clear-history-window")
                            .label("Clear")
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.model.update(cx, |model, cx| {
                                    model.history.clear();
                                    cx.notify();
                                });
                                cx.spawn(async move |_this, _cx| {
                                    let _ = smol::unblock(|| save_history(&[])).await;
                                })
                                .detach();
                            })),
                    ),
            )
            .child(
                div()
                    .id("history-window-scroll")
                    .flex_1()
                    .overflow_y_scroll()
                    .children(rows),
            )
    }
}

pub fn run(data: fn() -> HotData) {
    App::new()
        .setup(move |cx: &mut GpuiApp, _config| {
            gpui_component::init(cx);
            KitTheme::change(ThemeMode::Dark, None, cx);
            let model = cx.new(|_| InspectorModel {
                history: load_history(),
                ..InspectorModel::default()
            });
            cx.set_global(ModelGlobal(model));
        })
        .root_with::<Root>("main", move |window, cx| {
            let model = cx.global::<ModelGlobal>().0.clone();
            let view = cx.new(|cx| InspectorView::new(model, data, window, cx));
            cx.new(|cx| Root::new(view, window, cx))
        })
        .root_with::<Root>("history", |window, cx| {
            let model = cx.global::<ModelGlobal>().0.clone();
            let view = cx.new(|cx| HistoryView::new(model, cx));
            cx.new(|cx| Root::new(view, window, cx))
        })
        .run();
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write as _;
    use std::net::TcpListener;

    #[test]
    fn fetches_and_pretty_prints_json() {
        let listener = TcpListener::bind("127.0.0.1:0").expect("bind");
        let address = listener.local_addr().expect("address");
        let server = std::thread::spawn(move || {
            let (mut stream, _) = listener.accept().expect("accept");
            let mut request = [0u8; 2048];
            let _ = stream.read(&mut request);
            let body = "{\"ok\":true,\"items\":[1,2]}";
            write!(
                stream,
                "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                body.len(),
                body
            )
            .expect("response");
        });

        let response = perform_request("GET", &format!("http://{address}/"), "").expect("request");
        assert_eq!(response.status, 200);
        assert_eq!(response.reason, "OK");
        assert!(response.pretty.contains("\"ok\": true"));
        assert!(response.headers.to_lowercase().contains("content-type"));
        server.join().expect("server");
    }
}
