# Auro Inspector

A small native HTTP/API client built with [Auro](https://github.com/pdl-clay/auro) `0.1.0-alpha.1`.
It is the first real-app dogfood for the alpha: multiple windows, a form, asynchronous HTTP,
pretty-printed JSON, persistent history and response export, without a webview.

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
  (`~/.local/share/auro-inspector` when `XDG_DATA_HOME` is unset).
- Click a history row to load its method and URL.
- Export writes the raw response body to `$XDG_DATA_HOME/auro-inspector/exports/`.
- Two windows: the request view and a shared request-history window.
- The header title/accent is the hot boundary (`data/src`); editing it reloads in the same process.
  Editing this UI is a host change and restarts the app.

## Known limits

- No headers editor, auth, redirects policy, cookies, TLS controls, proxy or file dialog yet.
- History stores request metadata, not response bodies.
- The response pane renders at most 2 MiB and pretty-prints only valid JSON.
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
