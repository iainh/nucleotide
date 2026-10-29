//! Image tab lifecycle, remote image caching, dimensions, and rendering.

use std::hash::{DefaultHasher, Hash, Hasher};
use std::path::{Path, PathBuf};

use gpui::prelude::{FluentBuilder, StyledImage};
use gpui::{
    Bounds, Context, Hsla, InteractiveElement, IntoElement, ParentElement, ScrollHandle,
    StatefulInteractiveElement, Styled, canvas, div, img, px, relative,
};
use nucleotide_types::scrollbar::SCROLLBAR_THICKNESS;
use nucleotide_ui::ThemedContext as UIThemedContext;
use nucleotide_ui::scrollbar::{Scrollbar, ScrollbarState};
use nucleotide_ui::{Button, ButtonSize, ButtonVariant, Tooltipped};
use nucleotide_workspace::{ReadOptions, WorkspaceBackendHandle, WorkspaceIdentity};

use super::Workspace;
use crate::tab::TabId;

const IMAGE_ZOOM_STEP: f32 = 0.25;

const IMAGE_ZOOM_MIN: f32 = 0.10;

const IMAGE_ZOOM_MAX: f32 = 8.0;

const IMAGE_TRANSPARENCY_GRID_SIZE: f32 = 12.0;

const REMOTE_IMAGE_READ_LIMIT_BYTES: u64 = 128 * 1024 * 1024;

#[derive(Clone)]
pub(super) struct ImageTab {
    pub(super) id: u64,
    pub(super) path: PathBuf,
    pub(super) render_path: PathBuf,
    pub(super) dimensions: Option<(u32, u32)>,
    pub(super) focused_at: std::time::Instant,
    pub(super) zoom: f32,
    pub(super) scroll_handle: ScrollHandle,
    pub(super) vertical_scrollbar_state: ScrollbarState,
    pub(super) horizontal_scrollbar_state: ScrollbarState,
}

fn is_image_file_path(path: &Path) -> bool {
    path.extension()
        .and_then(|extension| extension.to_str())
        .map(|extension| {
            matches!(
                extension.to_ascii_lowercase().as_str(),
                "avif"
                    | "bmp"
                    | "dds"
                    | "exr"
                    | "farbfeld"
                    | "ff"
                    | "gif"
                    | "hdr"
                    | "ico"
                    | "jpeg"
                    | "jpg"
                    | "pam"
                    | "pbm"
                    | "pgm"
                    | "png"
                    | "ppm"
                    | "qoi"
                    | "svg"
                    | "tga"
                    | "tif"
                    | "tiff"
                    | "webp"
            )
        })
        .unwrap_or(false)
}

pub(super) fn should_open_with_image_viewer(
    path: &Path,
    has_initial_position: bool,
    backend_identity: &WorkspaceIdentity,
) -> bool {
    !has_initial_position
        && is_image_file_path(path)
        && matches!(
            backend_identity,
            WorkspaceIdentity::Local | WorkspaceIdentity::Remote(_)
        )
}

pub(super) fn remote_image_cache_dir() -> PathBuf {
    dirs::cache_dir()
        .unwrap_or_else(std::env::temp_dir)
        .join("nucleotide")
        .join("remote-images")
}

pub(super) fn remote_image_cache_path(
    backend_identity: &WorkspaceIdentity,
    path: &Path,
) -> PathBuf {
    let mut hasher = DefaultHasher::new();
    backend_identity.hash(&mut hasher);
    path.hash(&mut hasher);
    let hash = hasher.finish();
    let file_name = path
        .file_name()
        .and_then(|name| name.to_str())
        .filter(|name| !name.is_empty())
        .unwrap_or("image");
    remote_image_cache_dir().join(format!(
        "{hash:016x}-{}",
        sanitize_cache_file_name(file_name)
    ))
}

fn sanitize_cache_file_name(file_name: &str) -> String {
    file_name
        .chars()
        .map(|ch| {
            if ch.is_ascii_alphanumeric() || matches!(ch, '.' | '-' | '_') {
                ch
            } else {
                '_'
            }
        })
        .collect()
}

pub(super) async fn cache_remote_image_file(
    backend: WorkspaceBackendHandle,
    path: PathBuf,
) -> anyhow::Result<PathBuf> {
    let backend_identity = backend.identity();
    let read = backend
        .read_file(
            &path,
            ReadOptions {
                max_bytes: Some(REMOTE_IMAGE_READ_LIMIT_BYTES),
            },
        )
        .await?;

    if read.truncated {
        anyhow::bail!(
            "remote image is larger than {} MiB",
            REMOTE_IMAGE_READ_LIMIT_BYTES / (1024 * 1024)
        );
    }

    let cache_path = remote_image_cache_path(&backend_identity, &path);
    if let Some(parent) = cache_path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    std::fs::write(&cache_path, read.bytes)?;
    Ok(cache_path)
}

pub(super) fn image_zoom_percent(zoom: f32) -> String {
    format!("{:.0}%", zoom * 100.0)
}

fn image_transparency_grid_colors(editor_background: Hsla) -> (Hsla, Hsla) {
    if editor_background.l > 0.5 {
        (
            gpui::hsla(0.0, 0.0, 0.98, 1.0),
            gpui::hsla(0.0, 0.0, 0.86, 1.0),
        )
    } else {
        (
            gpui::hsla(0.0, 0.0, 0.16, 1.0),
            gpui::hsla(0.0, 0.0, 0.26, 1.0),
        )
    }
}

fn image_transparency_grid(base: Hsla, alternate: Hsla) -> gpui::AnyElement {
    let square_size = px(IMAGE_TRANSPARENCY_GRID_SIZE);

    div()
        .absolute()
        .size_full()
        .bg(base)
        .child(
            canvas(
                move |_, _, _| (),
                move |bounds, _, window, _| {
                    let rows = (bounds.size.height / square_size).ceil() as i32;
                    let cols = (bounds.size.width / square_size).ceil() as i32;

                    for row in 0..rows {
                        for col in 0..cols {
                            if (row + col) % 2 != 0 {
                                continue;
                            }

                            let origin = bounds.origin
                                + gpui::point(square_size * col as f32, square_size * row as f32);
                            window.paint_quad(gpui::fill(
                                Bounds {
                                    origin,
                                    size: gpui::size(square_size, square_size),
                                },
                                alternate,
                            ));
                        }
                    }
                },
            )
            .absolute()
            .size_full(),
        )
        .into_any_element()
}

fn image_file_dimensions(path: &Path) -> Option<(u32, u32)> {
    image::ImageReader::open(path)
        .ok()?
        .with_guessed_format()
        .ok()?
        .into_dimensions()
        .ok()
}

impl Workspace {
    fn image_tab_mut(&mut self, image_id: u64) -> Option<&mut ImageTab> {
        self.image_tabs.iter_mut().find(|tab| tab.id == image_id)
    }

    fn next_image_tab_id(&mut self) -> u64 {
        let id = self.next_image_tab_index;
        self.next_image_tab_index = self.next_image_tab_index.saturating_add(1);
        id
    }

    pub(super) fn open_image_file_internal(
        &mut self,
        path: &std::path::Path,
        should_focus: bool,
        cx: &mut Context<Self>,
    ) {
        self.open_image_file_with_render_path(path, path, should_focus, cx)
    }

    pub(super) fn open_image_file_with_render_path(
        &mut self,
        path: &std::path::Path,
        render_path: &std::path::Path,
        should_focus: bool,
        cx: &mut Context<Self>,
    ) {
        let path = path.to_path_buf();
        if let Some(tab) = self.image_tabs.iter_mut().find(|tab| tab.path == path) {
            tab.focused_at = std::time::Instant::now();
            self.active_image_tab_id = Some(tab.id);
        } else {
            let render_path = render_path.to_path_buf();
            let scroll_handle = ScrollHandle::new();
            let vertical_scrollbar_state = ScrollbarState::new(scroll_handle.clone());
            let horizontal_scrollbar_state = ScrollbarState::new(scroll_handle.clone());
            let tab = ImageTab {
                id: self.next_image_tab_id(),
                path: path.clone(),
                dimensions: image_file_dimensions(&render_path),
                render_path,
                focused_at: std::time::Instant::now(),
                zoom: 1.0,
                scroll_handle,
                vertical_scrollbar_state,
                horizontal_scrollbar_state,
            };
            self.active_image_tab_id = Some(tab.id);
            self.image_tabs.push(tab);
        }

        self.invalidate_tab_bar_documents();
        self.allow_tab_bar_auto_scroll();
        self.sync_pane_tabs(cx);

        if let Some(file_tree) = &self.file_tree {
            file_tree.update(cx, |tree, cx| {
                tree.sync_selection_with_file(Some(&path), cx);
            });
        }

        if should_focus {
            self.defer_focus_editor(cx);
        }

        cx.notify();
    }

    pub(super) fn switch_to_image_tab(&mut self, image_id: u64, cx: &mut Context<Self>) {
        if let Some(tab) = self.image_tab_mut(image_id) {
            tab.focused_at = std::time::Instant::now();
            self.active_image_tab_id = Some(image_id);
            self.invalidate_tab_bar_documents();
            self.allow_tab_bar_auto_scroll();
            self.sync_pane_tabs(cx);
            cx.notify();
        }
    }

    fn set_image_tab_zoom(&mut self, image_id: u64, zoom: f32, cx: &mut Context<Self>) {
        if let Some(tab) = self.image_tab_mut(image_id) {
            tab.zoom = zoom.clamp(IMAGE_ZOOM_MIN, IMAGE_ZOOM_MAX);
            cx.notify();
        }
    }

    pub(super) fn close_image_tab(
        &mut self,
        image_id: u64,
        activation_target: Option<TabId>,
        cx: &mut Context<Self>,
    ) {
        let pane = self.core.read(cx).editor.tree.focus;
        if self.detach_shared_pane_tab(pane, TabId::Image(image_id), activation_target, cx) {
            return;
        }
        let Some(index) = self.image_tabs.iter().position(|tab| tab.id == image_id) else {
            return;
        };

        self.image_tabs.remove(index);
        self.pinned_documents.remove(&TabId::Image(image_id));
        self.invalidate_tab_bar_documents();

        if self.active_image_tab_id == Some(image_id) {
            self.active_image_tab_id = None;
            if let Some(target_id) = activation_target {
                match target_id {
                    TabId::Image(image_id) => self.switch_to_image_tab(image_id, cx),
                    TabId::Document(doc_id) => self.switch_to_tab_document(doc_id, cx),
                }
            }
        }

        cx.notify();
    }

    pub(super) fn render_image_viewer(
        &self,
        tab: ImageTab,
        cx: &mut Context<Self>,
    ) -> gpui::AnyElement {
        let tokens = &cx.theme().tokens;
        let tab_id = tab.id;
        let zoom = tab.zoom;
        let image_path = tab.render_path.clone();
        let (grid_base, grid_alternate) = image_transparency_grid_colors(tokens.editor.background);
        let (image_element, image_size) = if let Some((width, height)) = tab.dimensions {
            let width = px(width as f32 * zoom);
            let height = px(height as f32 * zoom);
            (
                div()
                    .relative()
                    .flex_none()
                    .overflow_hidden()
                    .mx_auto()
                    .my_auto()
                    .w(width)
                    .h(height)
                    .child(image_transparency_grid(grid_base, grid_alternate))
                    .child(
                        img(image_path)
                            .object_fit(gpui::ObjectFit::Contain)
                            .w(width)
                            .h(height)
                            .flex_none(),
                    )
                    .into_any_element(),
                Some((width, height)),
            )
        } else {
            (
                div()
                    .relative()
                    .flex_none()
                    .overflow_hidden()
                    .mx_auto()
                    .my_auto()
                    .max_w_full()
                    .max_h_full()
                    .child(image_transparency_grid(grid_base, grid_alternate))
                    .child(
                        img(image_path)
                            .object_fit(gpui::ObjectFit::Contain)
                            .max_w_full()
                            .max_h_full(),
                    )
                    .into_any_element(),
                None,
            )
        };
        let image_scroll_body = if let Some((width, height)) = image_size {
            div()
                .flex()
                .w(width)
                .h(height)
                .min_w(relative(1.0))
                .min_h(relative(1.0))
                .child(image_element)
                .into_any_element()
        } else {
            div()
                .flex()
                .min_w(relative(1.0))
                .min_h(relative(1.0))
                .child(image_element)
                .into_any_element()
        };

        div()
            .id(format!("image-viewer-{tab_id:?}"))
            .flex()
            .flex_col()
            .size_full()
            .bg(tokens.editor.background)
            .child(
                div()
                    .id("image-viewer-toolbar")
                    .flex()
                    .flex_none()
                    .items_center()
                    .gap(tokens.sizes.space_2)
                    .px(tokens.sizes.space_3)
                    .h(crate::tab::tab_container_height(*tokens))
                    .border_b_1()
                    .border_color(tokens.chrome.border_default)
                    .bg(tokens.tab_bar_tokens().container_background)
                    .child(
                        Button::icon_only("image-zoom-out", "icons/zoom-out.svg")
                            .variant(ButtonVariant::Ghost)
                            .size(ButtonSize::Small)
                            .tooltip("Zoom Out")
                            .activate_on_mouse_down()
                            .disabled(zoom <= IMAGE_ZOOM_MIN)
                            .on_click({
                                let workspace = cx.entity().clone();
                                move |_event, _window, cx| {
                                    workspace.update(cx, |workspace, cx| {
                                        workspace.set_image_tab_zoom(
                                            tab_id,
                                            zoom - IMAGE_ZOOM_STEP,
                                            cx,
                                        );
                                    });
                                    cx.stop_propagation();
                                }
                            }),
                    )
                    .child(
                        Button::icon_only("image-zoom-reset", "icons/rotate-ccw.svg")
                            .variant(ButtonVariant::Ghost)
                            .size(ButtonSize::Small)
                            .tooltip("Zoom to 100%")
                            .activate_on_mouse_down()
                            .disabled((zoom - 1.0).abs() < f32::EPSILON)
                            .on_click({
                                let workspace = cx.entity().clone();
                                move |_event, _window, cx| {
                                    workspace.update(cx, |workspace, cx| {
                                        workspace.set_image_tab_zoom(tab_id, 1.0, cx);
                                    });
                                    cx.stop_propagation();
                                }
                            }),
                    )
                    .child(
                        Button::icon_only("image-zoom-in", "icons/zoom-in.svg")
                            .variant(ButtonVariant::Ghost)
                            .size(ButtonSize::Small)
                            .tooltip("Zoom In")
                            .activate_on_mouse_down()
                            .disabled(zoom >= IMAGE_ZOOM_MAX)
                            .on_click({
                                let workspace = cx.entity().clone();
                                move |_event, _window, cx| {
                                    workspace.update(cx, |workspace, cx| {
                                        workspace.set_image_tab_zoom(
                                            tab_id,
                                            zoom + IMAGE_ZOOM_STEP,
                                            cx,
                                        );
                                    });
                                    cx.stop_propagation();
                                }
                            }),
                    )
                    .child(
                        div()
                            .min_w(px(48.0))
                            .text_size(tokens.sizes.text_sm)
                            .text_color(tokens.chrome.text_chrome_secondary)
                            .child(image_zoom_percent(zoom)),
                    ),
            )
            .child({
                let scroll_content = div()
                    .id("image-viewer-scroll-content")
                    .size_full()
                    .min_w(px(0.0))
                    .min_h(px(0.0))
                    .overflow_scroll()
                    .track_scroll(&tab.scroll_handle)
                    .p(tokens.sizes.space_4)
                    .child(image_scroll_body);

                div()
                    .id("image-viewer-content")
                    .relative()
                    .flex_1()
                    .min_w(px(0.0))
                    .min_h(px(0.0))
                    .overflow_hidden()
                    .child(scroll_content)
                    .when_some(
                        Scrollbar::vertical(tab.vertical_scrollbar_state.clone()),
                        |container, scrollbar| {
                            container.child(
                                div()
                                    .id("image-viewer-vertical-scrollbar")
                                    .absolute()
                                    .top_0()
                                    .right_0()
                                    .bottom_0()
                                    .w(SCROLLBAR_THICKNESS)
                                    .child(scrollbar),
                            )
                        },
                    )
                    .when_some(
                        Scrollbar::horizontal(tab.horizontal_scrollbar_state.clone()),
                        |container, scrollbar| {
                            container.child(
                                div()
                                    .id("image-viewer-horizontal-scrollbar")
                                    .absolute()
                                    .left_0()
                                    .right_0()
                                    .bottom_0()
                                    .h(SCROLLBAR_THICKNESS)
                                    .child(scrollbar),
                            )
                        },
                    )
            })
            .into_any_element()
    }
}
