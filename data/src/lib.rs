use auro_inspector_types::{HotData, ViewData};

/// Hot boundary: edit this function and a running `[dev] hot = true` session reloads it.
///
/// This crate is deliberately tiny and has no framework dependency. Only the value changes; the
/// view, layout and event handlers live in the UI crate and stay compiled while this function is
/// replaced. Return the fixed [`HotData`] frame, not a new struct: the running host has to agree
/// with the patch on its layout.
pub fn view_data() -> HotData {
    HotData::encode(&ViewData {
        title: "Auro Inspector".to_owned(),
        accent: 0x0f172a,
    })
}
