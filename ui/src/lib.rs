//! Auro Inspector: a small native HTTP/API client.
//!
//! The request form, HTTP work, JSON rendering and persistence live in this host view. The
//! `data` crate remains the hot boundary: editing its title/accent reloads the header in the same
//! process, while editing this file is a host change and restarts the app.

use std::fs;
use std::io::Read as _;
use std::path::PathBuf;
use std::sync::Arc;
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

const MAX_RESPONSE_BYTES: u64 = 64 * 1024 * 1024;
const MAX_PREVIEW_BYTES: usize = 64 * 1024;
const MAX_HISTORY: usize = 100;
const MAX_SAVED_REQUESTS: usize = 50;

#[derive(Clone, Debug, Serialize, Deserialize)]
struct HistoryEntry {
    method: String,
    url: String,
    status: u16,
    duration_ms: u64,
    at: String,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
struct SavedRequest {
    method: String,
    url: String,
    body: String,
    at: String,
}

#[derive(Clone, Debug)]
struct ResponseView {
    status: u16,
    reason: String,
    headers: String,
    body: Arc<[u8]>,
    pretty: String,
    duration_ms: u64,
    size_bytes: usize,
    truncated: bool,
}

#[derive(Clone, Debug, Default)]
struct InspectorModel {
    history: Vec<HistoryEntry>,
    saved: Vec<SavedRequest>,
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

fn collection_path() -> PathBuf {
    data_dir().join("collection.json")
}

fn load_collection() -> Vec<SavedRequest> {
    let path = collection_path();
    let Ok(text) = fs::read_to_string(&path) else {
        return Vec::new();
    };
    serde_json::from_str(&text).unwrap_or_else(|error| {
        eprintln!(
            "auro-inspector: ignoring unreadable collection {}: {error}",
            path.display()
        );
        Vec::new()
    })
}

fn save_collection(requests: &[SavedRequest]) {
    let path = collection_path();
    let result = path
        .parent()
        .map_or(Ok(()), fs::create_dir_all)
        .and_then(|()| serde_json::to_vec_pretty(requests).map_err(std::io::Error::other))
        .and_then(|bytes| fs::write(&path, bytes));
    if let Err(error) = result {
        eprintln!("auro-inspector: collection write failed: {error}");
    }
}

fn truncate_preview(text: &str) -> String {
    let mut end = text.len().min(MAX_PREVIEW_BYTES);
    while !text.is_char_boundary(end) {
        end -= 1;
    }
    let mut preview = text[..end].to_owned();
    if text.len() > MAX_PREVIEW_BYTES {
        preview.push_str("\n... [preview truncated]");
    }
    preview
}

fn preview_body(body: &[u8]) -> String {
    let end = body.len().min(MAX_PREVIEW_BYTES);
    let mut preview = String::from_utf8_lossy(&body[..end]).into_owned();
    if body.len() > MAX_PREVIEW_BYTES {
        preview.push_str("\n... [preview truncated]");
    }
    preview
}

fn human_size(bytes: usize) -> String {
    const KIB: usize = 1024;
    const MIB: usize = 1024 * KIB;
    if bytes >= MIB {
        format!("{:.1} MiB", bytes as f64 / MIB as f64)
    } else if bytes >= KIB {
        format!("{:.1} KiB", bytes as f64 / KIB as f64)
    } else {
        format!("{bytes} B")
    }
}

fn export_body(body: &[u8]) -> Result<PathBuf, String> {
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
    let mut body = Vec::new();
    response
        .body_mut()
        .as_reader()
        .take(MAX_RESPONSE_BYTES + 1)
        .read_to_end(&mut body)
        .map_err(|error| error.to_string())?;
    let truncated = body.len() as u64 > MAX_RESPONSE_BYTES;
    if truncated {
        body.truncate(MAX_RESPONSE_BYTES as usize);
    }
    let size_bytes = body.len();
    let pretty = if size_bytes <= MAX_PREVIEW_BYTES {
        serde_json::from_slice::<Value>(&body)
            .ok()
            .and_then(|value| serde_json::to_string_pretty(&value).ok())
            .map(|text| truncate_preview(&text))
            .unwrap_or_else(|| preview_body(&body))
    } else {
        let mut preview = preview_body(&body);
        if truncated {
            preview.push_str("\n... [response larger than 64 MiB; body truncated]");
        }
        preview
    };
    Ok(ResponseView {
        status,
        reason,
        headers,
        body: Arc::from(body),
        pretty,
        duration_ms: started.elapsed().as_millis() as u64,
        size_bytes,
        truncated,
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
        let last = model.read(cx).saved.first().cloned();
        if let Some(last) = last {
            method.update(cx, |state, cx| {
                state.set_value(last.method.clone(), window, cx);
            });
            url.update(cx, |state, cx| {
                state.set_value(last.url.clone(), window, cx);
            });
            body.update(cx, |state, cx| {
                state.set_value(last.body.clone(), window, cx);
            });
        }
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

    fn load_saved(&mut self, request: &SavedRequest, window: &mut Window, cx: &mut Context<Self>) {
        self.method.update(cx, |state, cx| {
            state.set_value(request.method.clone(), window, cx);
        });
        self.url.update(cx, |state, cx| {
            state.set_value(request.url.clone(), window, cx);
        });
        self.body.update(cx, |state, cx| {
            state.set_value(request.body.clone(), window, cx);
        });
        self.model.update(cx, |model, cx| {
            model.message = Some("loaded saved request".to_owned());
            cx.notify();
        });
    }

    fn save_request(&mut self, cx: &mut Context<Self>) {
        let method = self.method.read(cx).value().trim().to_uppercase();
        let method = if method.is_empty() {
            "GET".to_owned()
        } else {
            method
        };
        let url = self.url.read(cx).value().trim().to_owned();
        if url.is_empty() {
            self.model.update(cx, |model, cx| {
                model.message = Some("enter a URL before saving".to_owned());
                cx.notify();
            });
            return;
        }
        let request = SavedRequest {
            method,
            url,
            body: self.body.read(cx).value().to_string(),
            at: Local::now().format("%Y-%m-%d %H:%M:%S").to_string(),
        };
        let mut saved = self.model.read(cx).saved.clone();
        saved.retain(|entry| {
            !(entry.method == request.method
                && entry.url == request.url
                && entry.body == request.body)
        });
        saved.insert(0, request);
        saved.truncate(MAX_SAVED_REQUESTS);
        self.model.update(cx, |model, cx| {
            model.saved = saved.clone();
            model.message = Some("saved request".to_owned());
            cx.notify();
        });
        cx.spawn(async move |_this, _cx| {
            let _ = smol::unblock(move || save_collection(&saved)).await;
        })
        .detach();
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
            let result = smol::unblock(move || export_body(response.body.as_ref())).await;
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

        let (history, saved, response, loading, message) = {
            let model = self.model.read(cx);
            (
                model.history.clone(),
                model.saved.clone(),
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

        let saved_rows = saved
            .iter()
            .enumerate()
            .map(|(index, request)| {
                let request = request.clone();
                h_flex()
                    .id(("saved", index))
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
                            .child(request.method.clone()),
                    )
                    .child(div().flex_1().child(request.url.clone()))
                    .on_click(cx.listener(move |this, _, window, cx| {
                        this.load_saved(&request, window, cx);
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
                        .child(format!(
                            "{} ms · {}{}",
                            response.duration_ms,
                            human_size(response.size_bytes),
                            if response.truncated {
                                " · 64 MiB cap"
                            } else {
                                ""
                            }
                        )),
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
                                KitButton::new("save")
                                    .label("Save")
                                    .on_click(cx.listener(|this, _, _, cx| this.save_request(cx))),
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
                                "GET is the default; Save keeps the request; click a history entry to load it.".to_owned()
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
                                    .px_3()
                                    .pt_3()
                                    .pb_1()
                                    .gap_2()
                                    .items_center()
                                    .child(div().text_size(px(16.0)).child("Saved"))
                                    .child(
                                        div()
                                            .text_size(px(11.0))
                                            .text_color(theme.muted_foreground)
                                            .child(saved.len().to_string()),
                                    ),
                            )
                            .child(
                                v_flex()
                                    .id("saved-scroll")
                                    .max_h(px(180.0))
                                    .overflow_y_scroll()
                                    .px_2()
                                    .when(saved.is_empty(), |this| {
                                        this.child(
                                            div()
                                                .p_2()
                                                .text_size(px(12.0))
                                                .text_color(theme.muted_foreground)
                                                .child("Save a request to keep it here."),
                                        )
                                    })
                                    .children(saved_rows),
                            )
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
                saved: load_collection(),
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
        assert!(response.body.len() > 0);
        server.join().expect("server");
    }

    #[test]
    fn saved_requests_round_trip_through_json() {
        let saved = vec![
            SavedRequest {
                method: "GET".to_owned(),
                url: "https://example.test/items".to_owned(),
                body: String::new(),
                at: "2026-11-19 03:00:00".to_owned(),
            },
            SavedRequest {
                method: "POST".to_owned(),
                url: "https://example.test/items".to_owned(),
                body: "{\"name\":\"x\"}".to_owned(),
                at: "2026-11-19 03:01:00".to_owned(),
            },
        ];
        let text = serde_json::to_string(&saved).expect("serialize");
        let decoded: Vec<SavedRequest> = serde_json::from_str(&text).expect("deserialize");
        assert_eq!(decoded, saved);
    }

    #[test]
    fn handles_a_50_mb_json_response_with_a_bounded_preview() {
        let listener = TcpListener::bind("127.0.0.1:0").expect("bind");
        let address = listener.local_addr().expect("address");
        let target = 50 * 1024 * 1024;
        let server = std::thread::spawn(move || {
            let (mut stream, _) = listener.accept().expect("accept");
            let mut request = [0u8; 2048];
            let _ = stream.read(&mut request);

            let mut body = String::with_capacity(target + 16);
            body.push_str("{\"items\":[");
            while body.len() < target {
                body.push_str("0,");
            }
            body.push_str("0]}");
            write!(
                stream,
                "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                body.len()
            )
            .expect("headers");
            stream.write_all(body.as_bytes()).expect("body");
        });

        let response = perform_request("GET", &format!("http://{address}/"), "").expect("request");
        assert_eq!(response.status, 200);
        assert!(
            response.size_bytes >= target,
            "got {} bytes, expected at least {}",
            response.size_bytes,
            target
        );
        assert_eq!(response.body.len(), response.size_bytes);
        assert!(response.pretty.len() <= MAX_PREVIEW_BYTES + 64);
        assert!(response.pretty.contains("\"items\""));
        server.join().expect("server");
    }
}
