//! The Modrinth browser: search, filters, sorting, the project-page overlay
//! and content installation into the active instance.

use std::sync::atomic::Ordering;

use crate::content;
use crate::icons;
use crate::updater;

use super::state::{
    base_mc_of, categories_for, fetch_icon_rgba, kind_uses_loader, strip_markdown, App,
    ContentPlatform, ContentSnapshot, ContentUi, IconState, COMMON_LICENSES,
};
use super::toast_ui::draw_search_icon;

impl App {
    /// The Modrinth tab: search content, pick a file, install it into the
    /// game directory. Tab state is accessed via `self.tab(platform)` to keep
    /// borrow conflicts out of the render closures.
    pub(crate) fn ui_content(&mut self, ui: &mut egui::Ui, platform: ContentPlatform) {
        ui.heading("Modrinth");
        ui.add_space(4.0);

        // Content kind tabs: mods are the default, the rest follow the
        // Modrinth project types.
        ui.horizontal_wrapped(|ui| {
            for kind in [
                content::ContentKind::Mod,
                content::ContentKind::ResourcePack,
                content::ContentKind::DataPack,
                content::ContentKind::Shader,
                content::ContentKind::Modpack,
                content::ContentKind::Plugin,
                content::ContentKind::Server,
                content::ContentKind::World,
            ] {
                ui.selectable_value(self.tab(platform).kind_slot(), kind, kind.label());
            }
        });
        ui.horizontal(|ui| {
            draw_search_icon(ui, 16.0, ui.visuals().text_color());
            let response = ui.add(
                egui::TextEdit::singleline(self.tab(platform).search_slot())
                    .hint_text("Search…")
                    .desired_width(260.0),
            );
            let target_mc = base_mc_of(&self.settings.selected_version);
            ui.weak(format!(
                "MC: {}",
                if target_mc.is_empty() {
                    "—"
                } else {
                    &target_mc
                }
            ));
            // Loader filter (mods only; Modrinth has loader facets).
            if kind_uses_loader(self.tab(platform).kind) {
                ui.separator();
                let loader_filter = self.tab(platform).loader_filter;
                ui.selectable_value(self.tab(platform).loader_slot(), None, "any loader");
                for l in updater::Loader::ALL {
                    ui.selectable_value(self.tab(platform).loader_slot(), Some(l), l.label());
                }
                let _ = loader_filter;
            }
            let search_pressed = ui.button("Search").clicked()
                || response.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter));
            if search_pressed {
                let tab = self.tab(platform);
                tab.results = None;
                tab.open_project = None;
                tab.files.clear();
            }
        });

        let installing = self.installing.load(Ordering::SeqCst) || self.content_downloading;
        let progress = self
            .content_progress
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .clone();
        if installing && !progress.is_empty() {
            ui.label(&progress);
        }
        ui.separator();

        // Lazily run the search.
        if self.tab(platform).results.is_none() && !self.tab(platform).loading {
            let kind = self.tab(platform).kind;
            let query = self.tab(platform).search.clone();
            let loader = self
                .tab(platform)
                .loader_filter
                .map(|l| l.slug().to_string());
            let mc = self.content_mc_filter.clone();
            let categories: Vec<String> = self.tab(platform).category_filter.clone();
            let license = self.tab(platform).license_filter.clone();
            let sort = self.tab(platform).sort;
            self.tab(platform).loading = true;
            self.spawn_job(
                move || {
                    content::search_modrinth(
                        &crate::net::agent(),
                        kind,
                        &query,
                        &mc,
                        loader.as_deref(),
                        &categories,
                        license.as_deref(),
                        sort,
                        30,
                    )
                    .map_err(|e| e.to_string())
                },
                move |app, result| {
                    let tab = app.tab(platform);
                    tab.results = Some(result);
                    tab.loading = false;
                },
            );
        }

        // Read-only snapshot of the state needed to render the results.
        let state = self.tab(platform).snapshot();
        let Some(results) = &state.results else {
            ui.weak("Type a query and press Search.");
            return;
        };
        let items = match results {
            Err(e) => {
                ui.colored_label(egui::Color32::YELLOW, e.to_string());
                return;
            }
            Ok(items) if items.is_empty() => {
                ui.weak("Nothing found.");
                return;
            }
            Ok(items) => items,
        };

        // Filter bar above the results: MC version, categories, license and
        // sort (Modrinth facets; CurseForge gets MC only).
        ui.horizontal_wrapped(|ui| {
            ui.weak("MC:");
            let mc_w = ui.available_width() * 0.13;
            ui.add(
                egui::TextEdit::singleline(&mut self.content_mc_filter)
                    .hint_text("any")
                    .desired_width(mc_w),
            );
            if platform == ContentPlatform::Modrinth {
                ui.separator();
                ui.weak("Sort:");
                egui::ComboBox::from_id_salt("content_sort")
                    .selected_text(state.sort.label())
                    .width(110.0)
                    .show_ui(ui, |ui| {
                        for s in [
                            content::SortIndex::Relevance,
                            content::SortIndex::Downloads,
                            content::SortIndex::Follows,
                            content::SortIndex::Newest,
                            content::SortIndex::Updated,
                        ] {
                            ui.selectable_value(&mut self.tab(platform).sort, s, s.label());
                        }
                    });
                if ui.button("Apply").clicked() {
                    self.tab(platform).results = None; // re-run the search
                }
                ui.separator();
                ui.weak("License:");
                egui::ComboBox::from_id_salt("content_license")
                    .selected_text(
                        self.tab(platform)
                            .license_filter
                            .clone()
                            .unwrap_or_else(|| "any".into()),
                    )
                    .width(110.0)
                    .show_ui(ui, |ui| {
                        ui.selectable_value(&mut self.tab(platform).license_filter, None, "any");
                        for lic in COMMON_LICENSES {
                            ui.selectable_value(
                                &mut self.tab(platform).license_filter,
                                Some(lic.to_string()),
                                *lic,
                            );
                        }
                    });
                // Category chips for the current kind.
                let cats = categories_for(state.kind);
                if !cats.is_empty() {
                    ui.separator();
                    for cat in cats {
                        let selected = state.category_filter.iter().any(|c| c == cat);
                        if ui.selectable_label(selected, *cat).clicked() {
                            let tab = self.tab(platform);
                            if selected {
                                tab.category_filter.retain(|c| c != cat);
                            } else {
                                tab.category_filter.push(cat.to_string());
                            }
                            tab.results = None;
                        }
                    }
                }
                if ui.button("Clear").clicked() {
                    let tab = self.tab(platform);
                    tab.category_filter.clear();
                    tab.license_filter = None;
                    tab.results = None;
                }
            }
        });
        ui.separator();

        egui::ScrollArea::vertical().show(ui, |ui| {
            for item in items {
                // Icon: fetch asynchronously through the shared cache; shows
                // a placeholder square while downloading (48px).
                let mut icon_tex = None;
                if !item.icon_url.is_empty() {
                    if !self.icon_cache.known(&item.icon_url) {
                        self.icon_cache
                            .insert(item.icon_url.clone(), IconState::Loading);
                        let url_task = item.icon_url.clone();
                        let url_key = item.icon_url.clone();
                        self.spawn_job(
                            move || fetch_icon_rgba(&url_task).map_err(|e| e.to_string()),
                            move |app, result| match result {
                                Ok((rgba, w, h)) => {
                                    let img = egui::ColorImage::from_rgba_unmultiplied(
                                        [w as usize, h as usize],
                                        &rgba,
                                    );
                                    let tex = app.ctx.load_texture(
                                        format!("icon:{}", url_key),
                                        img,
                                        egui::TextureOptions::LINEAR,
                                    );
                                    app.icon_cache.insert(url_key, IconState::Ready(tex));
                                }
                                Err(_) => {
                                    app.icon_cache.insert(url_key, IconState::Failed);
                                }
                            },
                        );
                    }
                    icon_tex = self.icon_cache.get(&item.icon_url).cloned();
                }

                // Card: icon + title + author/stats; LMB opens the page.
                ui.horizontal(|ui| {
                    let (card_rect, card_resp) = ui.allocate_exact_size(
                        egui::vec2(ui.available_width(), 48.0),
                        egui::Sense::click(),
                    );
                    let icon_rect = egui::Rect::from_min_size(
                        card_rect.left_top() + egui::vec2(2.0, 2.0),
                        egui::vec2(44.0, 44.0),
                    );
                    match &icon_tex {
                        Some(tex) => {
                            let img =
                                egui::Image::new(tex).fit_to_exact_size(egui::vec2(44.0, 44.0));
                            img.paint_at(ui, icon_rect);
                        }
                        None => {
                            ui.painter()
                                .rect_filled(icon_rect, 4.0, ui.visuals().faint_bg_color);
                        }
                    }
                    let text_rect = egui::Rect::from_min_max(
                        egui::pos2(icon_rect.right() + 8.0, card_rect.top() + 2.0),
                        egui::pos2(card_rect.right() - 4.0, card_rect.bottom() - 2.0),
                    );
                    let galley_title = ui.painter().layout(
                        item.title.clone(),
                        egui::FontId::proportional(15.0),
                        ui.visuals().text_color(),
                        text_rect.width(),
                    );
                    ui.painter().galley(
                        egui::pos2(text_rect.left(), text_rect.top()),
                        galley_title,
                        egui::Color32::TRANSPARENT,
                    );
                    let galley_sub = ui.painter().layout(
                        format!(
                            "by {} \u{b7} down {} \u{2665} {} [{}]",
                            item.author, item.downloads, item.follows, item.license
                        ),
                        egui::FontId::proportional(11.0),
                        ui.visuals().weak_text_color(),
                        text_rect.width(),
                    );
                    ui.painter().galley(
                        egui::pos2(text_rect.left(), text_rect.top() + 20.0),
                        galley_sub,
                        egui::Color32::TRANSPARENT,
                    );
                    if card_resp.clicked() {
                        let tab = self.tab(platform);
                        tab.open_project = Some(item.id.clone());
                    }
                    if card_resp.hovered() {
                        ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
                    }
                });
                ui.separator();
            }
        });

        // The open project renders as a full-screen overlay (like a site
        // page) above everything, with a back arrow in the top-left corner.
        if let Some(open_id) = state.open_project.clone() {
            if let Some(item) = items.iter().find(|i| i.id == open_id) {
                let ctx = self.ctx.clone();
                self.content_project_overlay(&ctx, platform, item, &state, installing);
            } else {
                self.tab(platform).open_project = None;
            }
        }
    }

    /// Full-screen project page overlay: dim + page panel on the top layer
    /// with a back arrow, so a click on a card opens the page like a site.
    pub(crate) fn content_project_overlay(
        &mut self,
        ctx: &egui::Context,
        platform: ContentPlatform,
        item: &content::ContentItem,
        state: &ContentSnapshot,
        installing: bool,
    ) {
        // Dim behind the overlay. Foreground sits above all panels.
        egui::Area::new(egui::Id::new("content_page_dim"))
            .order(egui::Order::Foreground)
            .fixed_pos(ctx.screen_rect().left_top())
            .show(ctx, |ui| {
                let screen = ctx.screen_rect();
                let resp = ui.allocate_rect(screen, egui::Sense::click());
                ui.painter()
                    .rect_filled(screen, 0.0, egui::Color32::from_black_alpha(160));
                if resp.clicked() {
                    self.tab(platform).open_project = None;
                }
            });

        // The page itself: above the dim, nearly full-screen with margins,
        // its own scroll. The back arrow (← Back) sits in the top bar.
        // Order::Foreground keeps the page ABOVE the dim area below it.
        egui::Window::new(" ")
            .id(egui::Id::new("content_page"))
            .order(egui::Order::Foreground)
            .anchor(egui::Align2::CENTER_CENTER, [0.0, 0.0])
            .fixed_size(ctx.screen_rect().size() - egui::vec2(48.0, 48.0))
            .resizable(false)
            .collapsible(false)
            .title_bar(false)
            .show(ctx, |ui| {
                ui.set_min_width(ui.available_width());
                // Top bar: back arrow + title + author + stats.
                ui.horizontal(|ui| {
                    let back = ui.button(format!("{} Back", icons::ARROW_BACK));
                    if back.clicked() || ui.input(|i| i.key_pressed(egui::Key::Escape)) {
                        self.tab(platform).open_project = None;
                    }
                    // Icon + title block.
                    let icon_url = item.icon_url.clone();
                    let icon_rect = ui
                        .allocate_exact_size(egui::vec2(56.0, 56.0), egui::Sense::hover())
                        .0;
                    match self.icon_cache.get(&icon_url) {
                        Some(tex) => {
                            egui::Image::new(tex)
                                .fit_to_exact_size(egui::vec2(56.0, 56.0))
                                .paint_at(ui, icon_rect);
                        }
                        None => {
                            ui.painter()
                                .rect_filled(icon_rect, 6.0, ui.visuals().faint_bg_color);
                        }
                    }
                    ui.vertical(|ui| {
                        ui.heading(&item.title);
                        ui.weak(format!(
                            "by {} · {} {} · {} {} · [{}]",
                            item.author,
                            icons::ARROW_DOWNWARD,
                            item.downloads,
                            icons::FAVORITE,
                            item.follows,
                            item.license
                        ));
                    });
                });
                ui.separator();

                egui::ScrollArea::vertical().show(ui, |ui| {
                    self.content_project_page(ui, platform, item, state, installing);
                });
            });
    }

    /// The full project page opened on left click: description, version
    /// list with sort orders (A-Z / Z-A / MC version / number), MC-version
    /// and Release/Beta/Alpha filters and colored R/B/A badges.
    pub(crate) fn content_project_page(
        &mut self,
        ui: &mut egui::Ui,
        platform: ContentPlatform,
        item: &content::ContentItem,
        state: &ContentSnapshot,
        installing: bool,
    ) {
        let project_id = item.id.clone();

        // Fetch versions once.
        if !state.files.contains_key(&project_id) && !state.files_loading {
            let id_task = project_id.clone();
            let mc = self.content_mc_filter.trim().to_string();
            let loader = self
                .tab(platform)
                .loader_filter
                .map(|l| l.slug().to_string());
            self.tab(platform).files_loading = true;
            let id_key = project_id.clone();
            self.spawn_job(
                move || {
                    content::modrinth_versions(
                        &crate::net::agent(),
                        &id_task,
                        &mc,
                        loader.as_deref(),
                    )
                    .map_err(|e| e.to_string())
                },
                move |app, result| {
                    let tab = app.tab(platform);
                    tab.files.insert(id_key, Some(result));
                    tab.files_loading = false;
                },
            );
        }
        // Fetch the page body lazily too.
        if !state.detail.contains_key(&project_id) {
            let id_task = project_id.clone();
            let id_key = project_id.clone();
            self.spawn_job(
                move || {
                    content::modrinth_project(&crate::net::agent(), &id_task)
                        .map_err(|e| e.to_string())
                },
                move |app, result| {
                    app.tab(platform).detail.insert(id_key, Some(result));
                },
            );
        }

        // Description body.
        match state.detail.get(&project_id) {
            Some(Some(Ok(detail))) => {
                egui::ScrollArea::vertical()
                    .max_height(160.0)
                    .show(ui, |ui| {
                        for line in detail.body.lines() {
                            let line = line.trim();
                            if line.is_empty() {
                                ui.add_space(2.0);
                            } else if line.starts_with('#') {
                                ui.strong(line.trim_start_matches('#').trim());
                            } else {
                                ui.label(strip_markdown(line));
                            }
                        }
                    });
            }
            _ => {
                ui.weak(&item.description);
            }
        }

        ui.add_space(4.0);
        ui.strong("Versions");
        let files_state = state.files.get(&project_id);
        match files_state {
            None => {
                ui.weak("Loading versions...");
            }
            Some(None) => {
                ui.weak("No versions.");
            }
            Some(Some(Err(e))) => {
                ui.colored_label(egui::Color32::YELLOW, e.to_string());
            }
            Some(Some(Ok(files))) => {
                // Sort + filter controls.
                ui.horizontal_wrapped(|ui| {
                    ui.weak("Sort:");
                    egui::ComboBox::from_id_salt(("vsort", project_id.as_str()))
                        .selected_text(self.tab(platform).version_sort.label())
                        .width(100.0)
                        .show_ui(ui, |ui| {
                            for s in content::VersionSort::ALL {
                                ui.selectable_value(
                                    self.tab(platform).version_sort_slot(),
                                    s,
                                    s.label(),
                                );
                            }
                        });
                    ui.separator();
                    ui.weak("MC:");
                    ui.add(
                        egui::TextEdit::singleline(self.tab(platform).version_mc_slot())
                            .hint_text("any")
                            .desired_width(110.0),
                    );
                    ui.separator();
                    // Release/Beta/Alpha chips.
                    for (label, value, color) in [
                        ("Release", "release", egui::Color32::from_rgb(0, 200, 0)),
                        ("Beta", "beta", egui::Color32::YELLOW),
                        ("Alpha", "alpha", egui::Color32::LIGHT_RED),
                    ] {
                        let sel = self
                            .tab(platform)
                            .version_type_filter
                            .iter()
                            .any(|t| t == value);
                        let text = egui::RichText::new(label).color(color);
                        if ui.selectable_label(sel, text).clicked() {
                            let tab = self.tab(platform);
                            if sel {
                                tab.version_type_filter.retain(|t| t != value);
                            } else {
                                tab.version_type_filter.push(value.to_string());
                            }
                        }
                    }
                });
                ui.separator();

                // Apply filter + sort to a local copy.
                let mut shown = files.clone();
                let mc_filter = self.tab(platform).version_mc_filter.trim().to_string();
                let types = self.tab(platform).version_type_filter.clone();
                shown = content::filter_versions(shown, &mc_filter, &types);
                content::sort_versions(&mut shown, self.tab(platform).version_sort);
                if shown.is_empty() {
                    ui.weak("No versions match the filters.");
                }
                for file in shown.iter().take(50) {
                    ui.horizontal(|ui| {
                        // Colored badge: R green, B yellow, A red.
                        let (letter, rgb) = content::version_badge(&file.version_type);
                        let (badge, _badge_resp) =
                            ui.allocate_exact_size(egui::vec2(20.0, 20.0), egui::Sense::hover());
                        let color = egui::Color32::from_rgb(rgb[0], rgb[1], rgb[2]);
                        let pf = ui.painter_at(badge);
                        pf.rect_filled(badge, 4.0, color.gamma_multiply(0.25));
                        pf.text(
                            badge.center(),
                            egui::Align2::CENTER_CENTER,
                            letter,
                            egui::FontId::proportional(13.0),
                            color,
                        );
                        ui.vertical(|ui| {
                            ui.monospace(&file.name);
                            ui.horizontal(|ui| {
                                if !file.author.is_empty() {
                                    ui.weak(format!("by {}", file.author));
                                }
                                if let Some(mc) = file.game_versions.first() {
                                    ui.weak(format!("MC {mc}"));
                                }
                                if !file.date_published.is_empty() {
                                    ui.weak(
                                        &file.date_published[..10.min(file.date_published.len())],
                                    );
                                }
                                ui.weak(format!("{:.1} MB", file.size as f32 / 1_048_576.0));
                            });
                        });
                        let label = if installing { "..." } else { "Download" };
                        if ui
                            .with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                                ui.add_enabled(!installing, egui::Button::new(label))
                            })
                            .inner
                            .clicked()
                        {
                            self.download_content(state.kind, item.clone(), file.clone());
                        }
                    });
                }
                if shown.len() > 50 {
                    ui.weak(format!("... {} more hidden", shown.len() - 50));
                }
            }
        }
    }
    pub(crate) fn tab(&mut self, platform: ContentPlatform) -> &mut ContentUi {
        match platform {
            ContentPlatform::Modrinth => &mut self.modrinth,
        }
    }

    /// Download one content file in the background.
    pub(crate) fn download_content(
        &mut self,
        kind: content::ContentKind,
        item: content::ContentItem,
        file: content::ContentFile,
    ) {
        self.content_downloading = true;
        *self
            .content_progress
            .lock()
            .unwrap_or_else(|e| e.into_inner()) = format!("downloading {}…", file.file_name);
        let game_dir = self.active_game_dir();
        // Data packs, shaders, plugins and server jars land in the user's
        // Downloads folder; mods and resource packs go into the game dir.
        let game_dir = if kind.goes_to_downloads() {
            dirs::download_dir().unwrap_or(game_dir)
        } else {
            game_dir
        };
        let progress = self.content_progress.clone();

        self.spawn_job(
            move || {
                let agent = crate::net::agent();
                let result = content::download_file(&agent, &game_dir, kind, &file);
                progress.lock().unwrap_or_else(|e| e.into_inner()).clear();
                result.map_err(|e| e.to_string())
            },
            move |app, result| {
                app.content_downloading = false;
                match result {
                    Ok(path) => {
                        let msg = format!("{} installed to {}", item.title, path.display());
                        app.log_console(format!("[RustLauncher] {msg}"));
                        app.notify_info(msg);
                    }
                    Err(e) => {
                        let code = "MODRINTH";
                        app.notify_error(code, e);
                    }
                }
            },
        );
    }
}
