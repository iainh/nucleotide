use crate::utils::color_to_hsla;
use crate::Core;
use gpui::*;
use helix_view::{DocumentId, ViewId};

#[derive(Clone)]
pub struct StatusLine {
    core: Entity<Core>,
    doc_id: DocumentId,
    view_id: ViewId,
    focused: bool,
    style: TextStyle,
    lsp_state: Option<Entity<crate::core::lsp_state::LspState>>,
}

impl StatusLine {
    pub fn new(
        core: Entity<Core>,
        doc_id: DocumentId,
        view_id: ViewId,
        focused: bool,
        style: TextStyle,
    ) -> Self {
        Self {
            core,
            doc_id,
            view_id,
            focused,
            style,
            lsp_state: None,
        }
    }
    
    pub fn with_lsp_state(mut self, lsp_state: Entity<crate::core::lsp_state::LspState>) -> Self {
        self.lsp_state = Some(lsp_state);
        self
    }

    fn style(&self, _window: &mut Window, cx: &mut App) -> (Hsla, Hsla) {
        let theme = cx.global::<crate::theme_manager::ThemeManager>().helix_theme();
        let base_style = if self.focused {
            theme.get("ui.statusline")
        } else {
            theme.get("ui.statusline.inactive")
        };
        let base_fg = base_style
            .fg
            .and_then(color_to_hsla)
            .unwrap_or(hsla(0.5, 0.5, 0.5, 1.));
        let base_bg = base_style
            .bg
            .and_then(color_to_hsla)
            .unwrap_or(hsla(0.5, 0.5, 0.5, 1.));
        (base_fg, base_bg)
    }
}

impl IntoElement for StatusLine {
    type Element = StatusLineElement;

    fn into_element(self) -> Self::Element {
        StatusLineElement(self)
    }
}

pub struct StatusLineElement(StatusLine);

impl IntoElement for StatusLineElement {
    type Element = Self;

    fn into_element(self) -> Self::Element {
        self
    }
}

impl Element for StatusLineElement {
    type RequestLayoutState = ();
    type PrepaintState = ();

    fn id(&self) -> Option<ElementId> {
        Some(ElementId::NamedInteger(
            format!("statusline-{}", self.0.doc_id).into(),
            0, // We'll use a fixed ID since we can't access ViewId's internal field
        ))
    }

    fn source_location(&self) -> Option<&'static core::panic::Location<'static>> {
        None
    }

    fn request_layout(
        &mut self,
        _global_id: Option<&GlobalElementId>,
        _inspector_id: Option<&InspectorElementId>,
        window: &mut Window,
        cx: &mut App,
    ) -> (LayoutId, Self::RequestLayoutState) {
        let mut style = Style::default();
        style.size.width = relative(1.).into();
        style.size.height = px(24.).into(); // Fixed height for status line
        let layout_id = window.request_layout(style, None, cx);
        (layout_id, ())
    }

    fn prepaint(
        &mut self,
        _global_id: Option<&GlobalElementId>,
        _inspector_id: Option<&InspectorElementId>,
        _bounds: Bounds<Pixels>,
        _request_layout: &mut Self::RequestLayoutState,
        _window: &mut Window,
        _cx: &mut App,
    ) -> Self::PrepaintState {
        // TODO: Calculate actual status line layout
    }

    fn paint(
        &mut self,
        _global_id: Option<&GlobalElementId>,
        _inspector_id: Option<&InspectorElementId>,
        bounds: Bounds<Pixels>,
        _request_layout: &mut Self::RequestLayoutState,
        _prepaint: &mut Self::PrepaintState,
        window: &mut Window,
        cx: &mut App,
    ) {
        // Get theme colors first to avoid borrow checker issues
        let (_fg_color, bg_color) = self.0.style(window, cx);
        
        // Collect all data we need before dropping the immutable borrow
        let (mode_name, file_name, position_text) = {
            let core = self.0.core.read(cx);
            let editor = &core.editor;
            let doc = match editor.document(self.0.doc_id) {
                Some(doc) => doc,
                None => return,
            };
            let view = match editor.tree.try_get(self.0.view_id) {
                Some(view) => view,
                None => return,
            };

            // Build status components
            let position = helix_core::coords_at_pos(
                doc.text().slice(..),
                doc.selection(view.id)
                    .primary()
                    .cursor(doc.text().slice(..)),
            );

            let mode_name = match editor.mode() {
                helix_view::document::Mode::Normal => "NOR",
                helix_view::document::Mode::Insert => "INS",
                helix_view::document::Mode::Select => "SEL",
            };

            let file_name: SharedString = doc.path()
                .map(|p| {
                    let path_str = p.to_string_lossy().to_string();
                    // Truncate long paths - keep filename and some parent directories
                    if path_str.len() > 50 {
                        if let Some(file_name) = p.file_name() {
                            let file_name_str = file_name.to_string_lossy();
                            if let Some(parent) = p.parent() {
                                if let Some(parent_name) = parent.file_name() {
                                    format!(".../{}/{}", parent_name.to_string_lossy(), file_name_str).into()
                                } else {
                                    format!(".../{}", file_name_str).into()
                                }
                            } else {
                                file_name_str.to_string().into()
                            }
                        } else {
                            "...".into()
                        }
                    } else {
                        path_str.into()
                    }
                })
                .unwrap_or_else(|| "[scratch]".into());

            let position_text = format!("{}:{}", position.row + 1, position.col + 1);
            
            (mode_name, file_name, position_text)
        };

        // Fill background
        window.paint_quad(gpui::fill(bounds, bg_color));

        // Create divider color with reduced opacity
        let divider_color = Hsla {
            h: _fg_color.h,
            s: _fg_color.s,
            l: _fg_color.l,
            a: 0.3,
        };

        // Shape the text runs for each status component
        let mode_run = TextRun {
            len: mode_name.len(),
            font: self.0.style.font(),
            color: _fg_color,
            background_color: None,
            strikethrough: None,
            underline: None,
        };
        
        let file_run = TextRun {
            len: file_name.len(),
            font: self.0.style.font(),
            color: _fg_color,
            background_color: None,
            strikethrough: None,
            underline: None,
        };
        
        let position_run = TextRun {
            len: position_text.len(),
            font: self.0.style.font(),
            color: _fg_color,
            background_color: None,
            strikethrough: None,
            underline: None,
        };
        
        // Layout the text elements with spacing
        let padding = px(8.);
        let divider_width = px(1.);
        let divider_height = px(16.);
        let gap = px(8.);
        
        // Calculate available width for file name (leave space for mode, position, and LSP)
        let mode_width = px(3. * 8.); // 3 chars for mode
        let position_width = px(10. * 8.); // Estimate for position (e.g., "999:999")
        
        // Calculate actual LSP text width
        let lsp_width = if let Some(lsp_state) = &self.0.lsp_state {
            if lsp_state.read(cx).status_message.is_some() {
                px(8.) // Single character width for spinner or space
            } else {
                px(0.) // No LSP indicator
            }
        } else {
            px(0.) // No LSP state
        };
        
        let reserved_width = padding * 2. + mode_width + position_width + lsp_width + gap * 6. + divider_width * 3.;
        let available_file_width = (bounds.size.width - reserved_width).max(px(50.)); // At least 50px for file name
        
        let mut x_offset = bounds.origin.x + padding;
        let y_center = bounds.origin.y + Pixels((bounds.size.height - self.0.style.line_height_in_pixels(px(16.0))).0 / 2.0);
        
        // Paint mode text
        let mode_text: SharedString = mode_name.into();
        let mode_line = window.text_system().shape_line(
            mode_text,
            self.0.style.font_size.to_pixels(px(16.0)),
            &[mode_run],
            None
        );
        if let Err(e) = mode_line.paint(gpui::Point::new(x_offset, y_center), self.0.style.line_height_in_pixels(px(16.0)), window, cx) {
            log::error!("Failed to paint mode text: {e:?}");
        }
        x_offset += mode_width + gap;
        
        // Paint first divider
        let divider_y = bounds.origin.y + Pixels((bounds.size.height - divider_height).0 / 2.0);
        window.paint_quad(fill(
            Bounds::new(
                gpui::Point::new(x_offset, divider_y),
                Size::new(divider_width, divider_height)
            ),
            divider_color
        ));
        x_offset += divider_width + gap;
        
        // Paint file name with clipping
        let file_start_x = x_offset;
        let file_line = window.text_system().shape_line(
            file_name.clone(),
            self.0.style.font_size.to_pixels(px(16.0)),
            &[file_run],
            None
        );
        
        // Set up clipping bounds for the file name
        let clip_bounds = Bounds::new(
            gpui::Point::new(file_start_x, bounds.origin.y),
            Size::new(available_file_width, bounds.size.height)
        );
        
        window.with_content_mask(Some(ContentMask { bounds: clip_bounds }), |window| {
            if let Err(e) = file_line.paint(gpui::Point::new(x_offset, y_center), self.0.style.line_height_in_pixels(px(16.0)), window, cx) {
                log::error!("Failed to paint file name: {e:?}");
            }
        });
        
        x_offset = file_start_x + available_file_width + gap;
        
        // Paint second divider
        window.paint_quad(fill(
            Bounds::new(
                gpui::Point::new(x_offset, divider_y),
                Size::new(divider_width, divider_height)
            ),
            divider_color
        ));
        x_offset += divider_width + gap;
        
        // Paint position text
        let position_shared: SharedString = position_text.into();
        let position_line = window.text_system().shape_line(
            position_shared,
            self.0.style.font_size.to_pixels(px(16.0)),
            &[position_run],
            None
        );
        if let Err(e) = position_line.paint(gpui::Point::new(x_offset, y_center), self.0.style.line_height_in_pixels(px(16.0)), window, cx) {
            log::error!("Failed to paint position text: {e:?}");
        }
        x_offset += position_width + gap;
        
        // Paint LSP status from actual state
        if let Some(lsp_state) = &self.0.lsp_state {
            if let Some(lsp_char) = lsp_state.read(cx).status_message.as_ref() {
                log::debug!("Painting LSP status: '{}'", lsp_char);
                
                // Paint divider before LSP status
                window.paint_quad(fill(
                    Bounds::new(
                        gpui::Point::new(x_offset, divider_y),
                        Size::new(divider_width, divider_height)
                    ),
                    divider_color
                ));
                x_offset += divider_width + gap;
                
                // Paint LSP status character (spinner or space)
                let lsp_shared: SharedString = lsp_char.clone().into();
                let lsp_run = TextRun {
                    len: lsp_shared.len(),
                    font: self.0.style.font(),
                    color: _fg_color,
                    background_color: None,
                    strikethrough: None,
                    underline: None,
                };
                
                let lsp_line = window.text_system().shape_line(
                    lsp_shared,
                    self.0.style.font_size.to_pixels(px(16.0)),
                    &[lsp_run],
                    None
                );
                
                if let Err(e) = lsp_line.paint(gpui::Point::new(x_offset, y_center), self.0.style.line_height_in_pixels(px(16.0)), window, cx) {
                    log::error!("Failed to paint LSP status: {e:?}");
                }
            } else {
                log::debug!("No LSP status message to paint");
            }
        } else {
            log::debug!("No LSP state available in statusline");
        }
    }
}

// TODO: Implement GPUI-based status line components as needed