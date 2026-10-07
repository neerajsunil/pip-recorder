//! Capture source enumeration, selection and the idle preview it starts.
use super::{
    encoding::update_encoding_labels,
    notify::{hwnd, notify, notify_issue},
    state::{Lock, Shared},
};
use crate::{MainWindow, SourceEntry};
use fastrecorder_core::SessionState;
use fastrecorder_platform as native;
use slint::ComponentHandle;

pub(super) fn refresh_sources(ui: &MainWindow, state: &Shared) {
    let mut state = state.locked();
    if state.session.state() != SessionState::Idle || !ui.get_capture_supported() {
        return;
    }
    match hwnd(ui).and_then(native::enumerate_sources) {
        Ok(sources) => {
            let selected = state.selected_candidate.as_ref().and_then(|selected| {
                sources
                    .iter()
                    .position(|candidate| candidate.same_target(selected))
            });
            ui.set_selected_source(selected.map_or(-1, |index| index as i32));
            if state.selected_candidate.is_some() && selected.is_none() {
                state.preview.take(); // Drop requests stop without blocking the UI.
                state.preview_channel = native::PreviewChannel::default();
                ui.set_preview_ready(false);
                ui.set_preview_image(slint::Image::default());
                ui.set_preview_error("Choose an available source.".into());
                state.source = None;
                state.selected_candidate = None;
                ui.set_source_selected(false);
                ui.set_source_name("".into());
                notify(ui, "The selected source is no longer available.", false);
            }
            let rows: Vec<SourceEntry> = sources
                .iter()
                .enumerate()
                .filter(|(_, source)| source.display == ui.get_display_tab())
                .map(|(index, source)| SourceEntry {
                    name: source.name.clone().into(),
                    detail: source.detail.clone().into(),
                    index: index as i32,
                })
                .collect();
            ui.set_sources(std::rc::Rc::new(slint::VecModel::from(rows)).into());
            state.sources = sources;
        }
        Err(error) => notify_issue(ui, "Couldn't find screens or windows.", error),
    }
}

pub(super) fn install(ui: &MainWindow, state: &Shared) {
    let weak = ui.as_weak();
    let source_state = state.clone();
    ui.on_refresh_sources(move || {
        if let Some(ui) = weak.upgrade() {
            refresh_sources(&ui, &source_state);
        }
    });

    let weak = ui.as_weak();
    let source_state = state.clone();
    ui.on_select_source(move |index| {
        let Some(ui) = weak.upgrade() else {
            return;
        };
        let mut state = source_state.locked();
        if state.session.state() != SessionState::Idle {
            return;
        }
        let Some(candidate) = usize::try_from(index)
            .ok()
            .and_then(|i| state.sources.get(i))
            .cloned()
        else {
            return;
        };
        match candidate.open() {
            Ok(source) => {
                ui.set_source_name(source.name.clone().into());
                ui.set_source_detail(format!("{} × {}", source.width, source.height).into());
                ui.set_source_selected(true);
                ui.set_source_is_display(candidate.display);
                ui.set_selected_source(index);
                state.preview.take();
                state.preview_channel = native::PreviewChannel::default();
                state.preview_channel.set_cursor(ui.get_capture_cursor());
                ui.set_preview_ready(false);
                ui.set_preview_error("".into());
                state.preview =
                    match native::Preview::start(source.clone(), state.preview_channel.clone()) {
                        Ok(preview) => Some(preview),
                        Err(error) => {
                            ui.set_preview_error(error.into());
                            None
                        }
                    };
                state.source = Some(source);
                update_encoding_labels(&ui, &state);
                ui.set_source_open(false);
                state.selected_candidate = Some(candidate);
                notify(&ui, "", false);
            }
            Err(error) => {
                state.preview.take();
                state.preview_channel = native::PreviewChannel::default();
                ui.set_preview_ready(false);
                ui.set_preview_error("Choose an available source.".into());
                state.source = None;
                state.selected_candidate = None;
                ui.set_source_selected(false);
                ui.set_selected_source(-1);
                notify_issue(&ui, "Choose another screen or window.", error);
            }
        }
    });
}
