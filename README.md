# Auro Inspector

A small native HTTP/API client built with [Auro](https://github.com/pdl-clay/auro) `0.1.0-alpha.1`.
It is the first real-app dogfood for the alpha: multiple windows, a form, asynchronous HTTP,
pretty-printed JSON, persistent history, saved requests and response export, without a webview.

## Run

```bash
cargo install auro-cli --version 0.1.0-alpha.1
auro dev
```

The app uses the published alpha (`auro = "0.1.0-alpha.1"`), not a path dependency.

## What works

- `GET`, `POST`, `PUT`, `PATCH` and `DELETE`; JSON request bodies for the body-carrying methods.
- URL, method and body fields; a response pane with status, headers, duration and pretty JSON.
- The last 100 requests are stored under `$XDG_DATA_HOME/auro-inspector/history.json`
  (`~/.local/share/auro-inspector` when `XDG_DATA_HOME` is unset). Click a history row to load it.
- **Save** stores the current method, URL and body in `collection.json`; the newest saved request
  is restored on startup, and the Saved list loads any entry back into the form.
- Export writes the raw response body to `$XDG_DATA_HOME/auro-inspector/exports/`.
- Responses are read off the UI thread. The body is capped at 64 MiB, the viewer previews at most
  64 KiB, and export writes the complete retained body. A 50 MiB JSON response is covered by a
  local-server test.
- Two windows: the request view and a shared request-history window, both declared in `Auro.toml`.
- The header title/accent is the hot boundary (`data/src`); editing it reloads in the same process.
  Editing this UI is a host change and restarts the app.

## Known limits

- No headers editor, auth, redirects policy, cookies, TLS controls, proxy or file dialog yet.
- History stores request metadata, not response bodies. Saved requests store the request body.
- Responses above 64 MiB are truncated for export; the viewer previews at most 64 KiB of any body.
  Pretty-printing is limited to bodies up to 64 KiB.
- Window sizes come from `Auro.toml`; runtime resize/position changes are not persisted.
- The updater is not enabled; this is an alpha dogfood, not a production app.

## Checks

```bash
cargo test
cargo check
```

The HTTP test uses a local TCP server; it does not touch the network.

## Clean-machine test

Run `auro new` in a separate empty directory, then `auro dev`, on a machine or account that has not
built Auro before. Record the time to the first window and any missing system package. That is the
external test tracked in the Auro repository issue #14.

### Result (2026-11-19, distrobox)

- Host: CachyOS, Docker 29.9.3, distrobox 1.8.2.5; container `auro-clean`, Ubuntu 22.04,
  separate empty HOME (`/home/pdl/auro-clean-home`).
- System packages: build-essential, curl, pkg-config, libxkbcommon(-x11)-dev, libwayland-dev,
  libx11-xcb-dev, libvulkan-dev, mesa-vulkan-drivers, libasound2-dev, libfontconfig1-dev,
  libssl-dev, libgtk-3-dev, libayatana-appindicator3-dev, libxcb*-dev, wayland-protocols.
- Rust 1.100.0 installed with rustup in the clean HOME.
- `cargo install auro-cli --version 0.1.0-alpha.1 --locked`: **2m58s**.
- `auro new auro-demo`: **0s**; `ui/Cargo.toml` has `auro = "0.1.0-alpha.1"` (registry mode).
- `auro doctor`: wayland/xkbcommon/x11/xcb/vulkan/alsa/fontconfig/openssl **ok**; `dx` missing,
  so hot reload is unavailable in this clean image (the default relaunch mode still works).
- `auro dev`: cold build + launch completed and the `auro-demo` window was observed by niri
  (PID 2981405) with `auro:startup`; the test was stopped after the window was up. The cold job
  wall time was 4m38s including dependency downloads and the GPUI build.
- Warm `auro dev` in the same clean container: **1.126s** to `auro:startup` (N=1, first-frame
  request, not a painted-frame measurement). The under-two-minute target is met on a warm build;
  the cold first build above is **not** under two minutes and is recorded separately.
- No missing system package after the list above; no Auro issue was needed for this run.

## Acceptance evidence

| Criterion | Evidence |
|---|---|
| Create, send, inspect and save a request without a terminal | The Save button persists method, URL and body to `$XDG_DATA_HOME/auro-inspector/collection.json`; the newest entry fills the form at startup and the Saved list loads any entry. `saved_requests_round_trip_through_json` passes. |
| A 50 MB JSON response does not block the UI | `handles_a_50_mb_json_response_with_a_bounded_preview` downloads more than 50 MiB from a local TCP server. The request runs in `smol::unblock`; the full retained body is `Arc<[u8]>`, and the viewer receives a preview capped at 64 KiB. The UI test suite completes in about one second. This is a data-path test, not a frame-time profile. |
| Restart restores the last collection and window layout | `collection.json` is loaded before the main view is built and the newest request is applied to the inputs. Both windows are declared in `Auro.toml` and opened at startup; the packaged run showed `Auro Inspector` and `Auro Inspector - History` under the same PID. |
| `auro build` and `auro bundle` produce a runnable Linux artifact | With the published `auro 0.1.0-alpha.1`: `auro build --dir .` succeeded, then `auro bundle --dir .` produced AppImage, deb and rpm. The extracted AppImage stayed running and niri exposed both windows under `com.pdl.auroinspector`. |
| Clean-machine window under two minutes | Warm run in the distrobox: 1.126s to `auro:startup`. The cold first build was 4m38s and is not claimed to meet the two-minute target. |

Packaged binaries embed `Auro.toml` and materialise it in the per-user data directory when no config is present next to the executable. This compensates for the current bundler not placing the runtime config in the package; it is recorded as an Auro packaging gap, not hidden.

## License

Dual-licensed under either [MIT](LICENSE-MIT) or [Apache-2.0](LICENSE-APACHE), at your option.

