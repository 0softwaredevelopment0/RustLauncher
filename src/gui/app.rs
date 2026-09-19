//! The eframe entry point: the frame-update loop, navigation sidebar and the
//! confirmation dialogs that overlay any screen.

use std::sync::atomic::Ordering;

use crate::diagnostics;
use crate::icons;
use crate::lang::{Language, tr, tr_fmt};
use crate::news::{self, NewsItem};
use crate::settings;

use super::state::{App, ContentPlatform, Screen};

impl App {
    pub(crate) fn ui_news(&mut self, ui: &mut egui::Ui) {
        let lang = self.settings.language;
        // Detail view: single article.
        if let Some(idx) = self.news_selected {
            let item = match &self.news {
                Some(Ok(items)) => items.get(idx).cloned(),
                _ => None,
            };
            if let Some(item) = item {
                ui.horizontal(|ui| {
                    if ui
                        .button(format!("{} {}", icons::ARROW_BACK, tr(lang, "Back")))
                        .clicked()
                    {
                        self.news_selected = None;
                    }
                });
                ui.add_space(4.0);
                self.news_detail(ui, &item);
                return;
            }
            self.news_selected = None;
        }

        // Card grid.
        ui.heading(tr(lang, "News"));
        ui.add_space(4.0);
        if ui.button(tr(lang, "Refresh")).clicked() {
            self.news = None;
            self.news_selected = None;
            self.fetch_news();
        }
        ui.separator();
        match &self.news {
            None => {
                ui.label(tr(lang, "Loading…"));
            }
            Some(Err(e)) => {
                ui.colored_label(
                    egui::Color32::LIGHT_RED,
                    tr_fmt(lang, "Feed unavailable: {0}", &[&e.to_string()]),
                );
                ui.add_space(4.0);
                for item in news::fallback_news(lang) {
                    self.news_card(ui, &item, None);
                }
            }
            Some(Ok(items)) => {
                let items = items.clone();
                egui::ScrollArea::vertical().show(ui, |ui| {
                    egui::Grid::new("news_grid")
                        .num_columns(2)
                        .min_col_width((ui.available_width() / 2.0).max(220.0))
                        .spacing([12.0, 12.0])
                        .show(ui, |ui| {
                            for (idx, item) in items.iter().enumerate() {
                                self.news_card(ui, item, Some(idx));
                                if idx % 2 == 1 {
                                    ui.end_row();
                                }
                            }
                            if items.len() % 2 == 1 {
                                ui.label("");
                                ui.end_row();
                            }
                        });
                });
            }
        }
    }

    fn news_card(&mut self, ui: &mut egui::Ui, item: &NewsItem, idx: Option<usize>) {
        let lang = self.settings.language;
        let card_bg = egui::Color32::from_rgba_premultiplied(30, 30, 40, 200);
        let border_color = egui::Color32::from_rgba_premultiplied(60, 60, 80, 200);

        let frame = egui::Frame::none()
            .fill(card_bg)
            .rounding(8.0)
            .inner_margin(egui::Margin::symmetric(14.0, 12.0))
            .stroke(egui::Stroke::new(1.0_f32, border_color));

        let resp = frame
            .show(ui, |ui| {
                // Top row: author + date.
                ui.horizontal(|ui| {
                    let name = item.author_name(lang);
                    ui.label(egui::RichText::new(name).small().strong());
                    ui.label(
                        egui::RichText::new(item.formatted_date())
                            .small()
                            .color(egui::Color32::GRAY),
                    );
                });
                ui.add_space(6.0);
                // Title.
                ui.label(egui::RichText::new(&item.title).strong().size(15.0));
                ui.add_space(4.0);
                // Content preview (max 3 lines).
                let preview: String = item.content.lines().take(3).collect::<Vec<_>>().join("\n");
                ui.label(
                    egui::RichText::new(preview)
                        .small()
                        .color(egui::Color32::from_rgb(180, 180, 190)),
                );
                ui.add_space(4.0);
                ui.label(
                    egui::RichText::new(format!("{}  {}", icons::CHEVRON_RIGHT, tr(lang, "Read more")))
                        .small()
                        .strong()
                        .color(egui::Color32::from_rgb(0, 200, 255)),
                );
            })
            .response;

        // Whole card is clickable — covers the title, preview and the
        // "Read more" label. This also makes fallback cards (news feed
        // unreachable) clickable, which previously did nothing.
        let idx_slot = idx.map(|i| i as u64).unwrap_or(u64::MAX);
        let card = ui.interact(
            resp.rect,
            egui::Id::new(("news_card", idx_slot)),
            egui::Sense::click(),
        );
        if card.hovered() {
            ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
        }
        if card.clicked() && idx.is_some() {
            self.news_selected = idx;
        }
        ui.add_space(8.0);
    }

    fn news_detail(&self, ui: &mut egui::Ui, item: &NewsItem) {
        let lang = self.settings.language;
        // Author + date header.
        ui.horizontal(|ui| {
            let name = item.author_name(lang);
            ui.label(egui::RichText::new(name).strong().size(14.0));
            ui.separator();
            ui.label(egui::RichText::new(item.formatted_date()).color(egui::Color32::GRAY));
        });
        ui.add_space(8.0);
        // Title.
        ui.label(egui::RichText::new(&item.title).strong().size(20.0));
        ui.add_space(12.0);
        ui.separator();
        ui.add_space(8.0);
        // Full content.
        for line in item.content.lines() {
            ui.label(line);
        }
        if item.content.is_empty() {
            ui.label(egui::RichText::new(tr(lang, "No content")).color(egui::Color32::GRAY));
        }
    }

    pub(crate) fn ui_settings(&mut self, ui: &mut egui::Ui) {
        let lang = self.settings.language;
        ui.heading(tr(lang, "Settings"));
        ui.add_space(4.0);
        // Java-mode checkboxes mirror `use_custom_java`; only one is checked.
        let mut default_java = !self.settings.use_custom_java;
        let mut custom_java = self.settings.use_custom_java;
        egui::ScrollArea::vertical().show(ui, |ui| {
            // Language first: it applies immediately, no save needed.
            ui.strong(tr(lang, "Language"));
            ui.horizontal(|ui| {
                let mut language = self.settings.language;
                egui::ComboBox::from_id_salt("language")
                    .selected_text(language.label())
                    .width(140.0)
                    .show_ui(ui, |ui| {
                        for l in Language::ALL {
                            ui.selectable_value(&mut language, l, l.label());
                        }
                    });
                if language != self.settings.language {
                    self.settings.language = language;
                }
            });
            ui.label(
                egui::RichText::new(tr(lang, "Applies immediately."))
                    .small()
                    .color(egui::Color32::GRAY),
            );
            // Java: Default vs Custom, switched with checkboxes.
            ui.strong(tr(lang, "Java"));
            ui.horizontal(|ui| {
                if ui
                    .checkbox(&mut default_java, tr(lang, "Default (auto-detect)"))
                    .changed()
                    && default_java
                {
                    self.settings.use_custom_java = false;
                }
                if ui.checkbox(&mut custom_java, tr(lang, "Custom path")).changed() && custom_java
                {
                    self.settings.use_custom_java = true;
                }
            });
            ui.add_enabled_ui(self.settings.use_custom_java, |ui| {
                ui.horizontal(|ui| {
                    ui.label(tr(lang, "Java executable"));
                    ui.add_enabled(
                        true,
                        egui::TextEdit::singleline(&mut self.settings.java_path)
                            .desired_width(360.0),
                    );
                    if ui.button("…").clicked() {
                        if let Some(file) = rfd::FileDialog::new().pick_file() {
                            self.settings.java_path = file.to_string_lossy().to_string();
                        }
                    }
                });
            });
            ui.add_space(4.0);

            // JVM flags: presets + free edit; heap flags are mandatory.
            ui.strong(tr(lang, "JVM flags"));
            ui.horizontal(|ui| {
                for preset in crate::jvm::PRESETS {
                    let selected = self.settings.java_args == preset.args;
                    if ui
                        .add_enabled(!selected, egui::Button::new(preset.label))
                        .clicked()
                    {
                        self.settings.java_args = preset.args.to_string();
                    }
                }
            });
            ui.add(
                egui::TextEdit::multiline(&mut self.settings.java_args)
                    .desired_rows(3)
                    .desired_width(f32::INFINITY)
                    .font(egui::TextStyle::Monospace),
            );
            {
                let flags: Vec<String> = self
                    .settings
                    .java_args
                    .split_whitespace()
                    .map(|s| s.trim().to_string())
                    .filter(|s| !s.is_empty())
                    .collect();
                match crate::jvm::validate_jvm_args(&flags, lang) {
                    Ok(()) => {
                        ui.colored_label(
                            egui::Color32::LIGHT_GREEN,
                            format!("{} {}", icons::CHECK, tr(lang, "heap flags present")),
                        );
                    }
                    Err(e) => {
                        ui.colored_label(egui::Color32::LIGHT_RED, e);
                    }
                }
            }
            ui.add_space(4.0);

            ui.strong(tr(lang, "Game"));
            ui.checkbox(
                &mut self.settings.use_custom_resolution,
                tr(lang, "Custom resolution"),
            );
            ui.add_enabled_ui(self.settings.use_custom_resolution, |ui| {
                ui.horizontal(|ui| {
                    ui.label(tr(lang, "Width"));
                    ui.add(egui::DragValue::new(&mut self.settings.game_width).range(320..=7680));
                    ui.label(tr(lang, "Height"));
                    ui.add(egui::DragValue::new(&mut self.settings.game_height).range(240..=4320));
                });
            });

            ui.strong(tr(lang, "Logs"));
            ui.horizontal(|ui| {
                ui.label(tr(lang, "Log to file:"));
                egui::ComboBox::from_id_salt("file_log")
                    .selected_text(self.settings.file_log.label(lang))
                    .width(170.0)
                    .show_ui(ui, |ui| {
                        for mode in settings::FileLogMode::ALL {
                            ui.selectable_value(
                                &mut self.settings.file_log,
                                mode,
                                mode.label(lang),
                            );
                        }
                    });
            });
            ui.label(
                egui::RichText::new(tr(
                    lang,
                    "What the logs/ files (launcher + game) capture. \
                     What the Console tab shows is picked on the Console tab.",
                ))
                .small()
                .color(egui::Color32::GRAY),
            );
            ui.strong(tr(lang, "Customization"));
            ui.horizontal(|ui| {
                ui.label(tr(lang, "Theme:"));
                let mut preset = self.settings.theme.preset;
                egui::ComboBox::from_id_salt("theme_preset")
                    .selected_text(preset.label(lang))
                    .width(110.0)
                    .show_ui(ui, |ui| {
                        for p in settings::ThemePreset::ALL {
                            ui.selectable_value(&mut preset, p, p.label(lang));
                        }
                    });
                if preset != self.settings.theme.preset {
                    self.settings.theme = settings::Theme::from_preset(preset);
                    // Drop the decoded photo so a later Image mode reloads it.
                    self.bg_texture = None;
                }
            });
            ui.horizontal(|ui| {
                ui.label(tr(lang, "Background:"));
                for mode in settings::BackgroundMode::ALL {
                    if ui
                        .radio_value(&mut self.settings.theme.background, mode, mode.label(lang))
                        .changed()
                    {
                        self.settings.theme.preset = settings::ThemePreset::Custom;
                    }
                }
            });
            match self.settings.theme.background {
                settings::BackgroundMode::Color => {
                    ui.horizontal(|ui| {
                        ui.label(tr(lang, "Color:"));
                        if ui
                            .color_edit_button_srgb(&mut self.settings.theme.bg_color)
                            .changed()
                        {
                            self.settings.theme.preset = settings::ThemePreset::Custom;
                        }
                    });
                }
                settings::BackgroundMode::Gradient => {
                    ui.horizontal(|ui| {
                        ui.label(tr(lang, "Top:"));
                        if ui
                            .color_edit_button_srgb(&mut self.settings.theme.bg_top)
                            .changed()
                        {
                            self.settings.theme.preset = settings::ThemePreset::Custom;
                        }
                        ui.label(tr(lang, "Bottom:"));
                        if ui
                            .color_edit_button_srgb(&mut self.settings.theme.bg_bottom)
                            .changed()
                        {
                            self.settings.theme.preset = settings::ThemePreset::Custom;
                        }
                    });
                }
                settings::BackgroundMode::Image => {
                    ui.horizontal(|ui| {
                        ui.label(tr(lang, "Image:"));
                        let edit = ui.add(
                            egui::TextEdit::singleline(&mut self.settings.theme.bg_image)
                                .desired_width(280.0),
                        );
                        if edit.changed() {
                            self.settings.theme.preset = settings::ThemePreset::Custom;
                        }
                        if ui.button("…").clicked() {
                            if let Some(file) = rfd::FileDialog::new()
                                .add_filter("Image", &["png", "jpg", "jpeg"])
                                .pick_file()
                            {
                                self.settings.theme.bg_image =
                                    file.to_string_lossy().to_string();
                                self.settings.theme.preset = settings::ThemePreset::Custom;
                            }
                        }
                    });
                    ui.label(
                        egui::RichText::new(tr(lang, "PNG or JPG photo, stretched to cover the window."))
                            .small()
                            .color(egui::Color32::GRAY),
                    );
                }
            }
            ui.horizontal(|ui| {
                ui.label(tr(lang, "Buttons:"));
                if ui
                    .color_edit_button_srgb(&mut self.settings.theme.button)
                    .changed()
                {
                    self.settings.theme.preset = settings::ThemePreset::Custom;
                }
                ui.label(tr(lang, "Accent:"));
                if ui
                    .color_edit_button_srgb(&mut self.settings.theme.accent)
                    .changed()
                {
                    self.settings.theme.preset = settings::ThemePreset::Custom;
                }
                if ui
                    .checkbox(&mut self.settings.theme.dark_base, tr(lang, "Dark base"))
                    .changed()
                {
                    self.settings.theme.preset = settings::ThemePreset::Custom;
                }
            });
            ui.label(
                egui::RichText::new(tr(
                    lang,
                    "Accent colors selection, links and pressed buttons. \
                     Any manual tweak switches the theme to Custom.",
                ))
                .small()
                .color(egui::Color32::GRAY),
            );

            ui.strong(tr(lang, "News"));
            ui.horizontal(|ui| {
                ui.label(tr(lang, "News site URL"));
                ui.add(
                    egui::TextEdit::singleline(&mut self.settings.news_url).desired_width(360.0),
                );
                ui.label(
                    egui::RichText::new(tr(lang, "Website base URL used for the news feed"))
                        .small()
                        .color(egui::Color32::GRAY),
                );
            });
        });
        ui.separator();
        ui.horizontal(|ui| {
            if ui
                .button(format!("{} {}", icons::CHECK, tr(lang, "Save settings")))
                .clicked()
            {
                self.save_settings();
                self.reload_versions();
                self.play_status = tr(lang, "Settings saved").into();
            }
            if ui
                .button(format!("{} {}", icons::RESTORE, tr(lang, "Reset settings")))
                .clicked()
            {
                self.settings_reset_pending = true;
            }
        });
    }

    pub(crate) fn ui_diagnostics(&mut self, ui: &mut egui::Ui) {
        let lang = self.settings.language;
        ui.heading(tr(lang, "Network diagnostics"));
        ui.add_space(4.0);
        ui.horizontal(|ui| {
            if ui
                .add_enabled(!self.diag_running, egui::Button::new(tr(lang, "Run tests")))
                .clicked()
            {
                self.diag_running = true;
                let lang = self.settings.language;
                self.spawn_job(
                    move || diagnostics::run_all(&crate::net::agent(), lang),
                    |app, results| {
                        app.diag_results = Some(results);
                        app.diag_running = false;
                    },
                );
            }
            if self.diag_running {
                ui.label(tr(lang, "Testing…"));
            }
        });
        ui.separator();
        match &self.diag_results {
            None => {
                ui.weak(tr(
                    lang,
                    "Press Run tests to check DNS, HTTP and TCP connectivity.",
                ));
            }
            Some(results) => {
                egui::ScrollArea::vertical().show(ui, |ui| {
                    for result in results {
                        ui.horizontal(|ui| {
                            let color = if result.ok {
                                egui::Color32::LIGHT_GREEN
                            } else {
                                egui::Color32::LIGHT_RED
                            };
                            ui.colored_label(color, if result.ok { "OK  " } else { "FAIL" });
                            ui.monospace(&result.name);
                            ui.weak(&result.detail);
                        });
                    }
                });
            }
        }
    }
}
impl eframe::App for App {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        self.apply_pending_jobs();
        self.sync_visuals(ctx);
        self.paint_background(ctx);
        self.tick_toasts();

        // Repaint while the game runs or a download is in progress so the
        // console and progress labels keep moving without user input.
        let busy = self.any_game_running() || self.installing.load(Ordering::SeqCst);
        if busy {
            ctx.request_repaint_after(std::time::Duration::from_millis(120));
        }

        // Toasts animate (slide-out) and must keep repainting while visible.
        if !self.toasts.is_empty() {
            ctx.request_repaint_after(std::time::Duration::from_millis(66));
        }

        egui::SidePanel::left("nav").show(ctx, |ui| {
            let lang = self.settings.language;
            ui.add_space(8.0);
            ui.heading("RustLauncher");
            ui.add_space(8.0);
            for (screen, label) in [
                (Screen::General, format!("{}  {}", icons::PLAY_ARROW, tr(lang, "General"))),
                (Screen::Console, format!("{}  {}", icons::TERMINAL, tr(lang, "Console"))),
                (
                    Screen::Instances,
                    format!("{}  {}", icons::VIDEOGAME_ASSET, tr(lang, "Instances")),
                ),
                (Screen::Versions, format!("{}  {}", icons::LAYERS, tr(lang, "Versions"))),
                (Screen::Servers, format!("{}  {}", icons::DNS, tr(lang, "Servers"))),
                (
                    Screen::Accounts,
                    format!("{}  {}", icons::ACCOUNT_CIRCLE, tr(lang, "Accounts")),
                ),
                (Screen::Skins, format!("{}  {}", icons::FACE, tr(lang, "Skins"))),
                (Screen::Modrinth, format!("{}  {}", icons::EXTENSION, tr(lang, "Modrinth"))),
                (Screen::News, format!("{}  {}", icons::ARTICLE, tr(lang, "News"))),
                (Screen::Settings, format!("{}  {}", icons::SETTINGS, tr(lang, "Settings"))),
                (
                    Screen::Diagnostics,
                    format!("{}  {}", icons::BUILD, tr(lang, "Diagnostics")),
                ),
            ] {
                let selected = self.screen == screen;
                if ui.selectable_label(selected, label).clicked() {
                    self.screen = screen;
                    if screen == Screen::Skins {
                        self.refresh_skins();
                        self.select_saved_skin();
                    }
                }
            }
            ui.separator();
            if let Some(account) = self.accounts.current() {
                ui.label(format!("{}: {}", tr(lang, "Player"), account.username));
            }
            ui.label(format!(
                "{}: {}",
                tr(lang, "Version"),
                self.settings.selected_version
            ));
            if self.any_game_running() {
                ui.colored_label(egui::Color32::LIGHT_GREEN, tr(lang, "Game running"));
            }
        });

        self.show_toasts(ctx);

        if self.account_remove_pending.is_some() {
            self.show_account_removal(ctx);
        }

        if self.instance_delete_pending.is_some() {
            self.show_instance_delete_confirmation(ctx);
        }
        if let Some((pid_handle, kind)) = self.instance_terminate_pending.clone() {
            self.show_instance_terminate_confirmation(ctx, &pid_handle, kind);
        }

        if self.settings_reset_pending {
            self.show_settings_reset_confirmation(ctx);
        }

        egui::CentralPanel::default().show(ctx, |ui| match self.screen {
            Screen::General => self.ui_general(ui),
            Screen::Console => self.ui_console(ui),
            Screen::Instances => self.ui_instances(ui),
            Screen::Versions => self.ui_versions(ui),
            Screen::Servers => self.ui_servers(ui),
            Screen::Accounts => self.ui_accounts(ui),
            Screen::Skins => self.ui_skins(ui, ctx),
            Screen::Modrinth => self.ui_content(ui, ContentPlatform::Modrinth),
            Screen::News => self.ui_news(ui),
            Screen::Settings => self.ui_settings(ui),
            Screen::Diagnostics => self.ui_diagnostics(ui),
        });
    }
}

// ── settings reset confirmation ───────────────────────────────

impl App {
    /// "Reset all settings to defaults?" dialog.
    fn show_settings_reset_confirmation(&mut self, ctx: &egui::Context) {
        let screen = ctx.screen_rect();
        let lang = self.settings.language;

        egui::Area::new(egui::Id::new("settings_reset_dim"))
            .order(egui::Order::Middle)
            .fixed_pos(screen.left_top())
            .show(ctx, |ui| {
                let resp = ui.allocate_rect(screen, egui::Sense::click());
                ui.painter()
                    .rect_filled(screen, 0.0, egui::Color32::from_black_alpha(140));
                if resp.clicked() {
                    self.settings_reset_pending = false;
                }
            });

        egui::Window::new(egui::RichText::new(tr(lang, "Confirm")).strong())
            .id(egui::Id::new("settings_reset_dialog"))
            .order(egui::Order::Foreground)
            .anchor(egui::Align2::CENTER_CENTER, [0.0, 0.0])
            .collapsible(false)
            .resizable(false)
            .title_bar(false)
            .fixed_size(egui::vec2(400.0, 190.0))
            .show(ctx, |ui| {
                ui.horizontal(|ui| {
                    super::toast_ui::draw_warning_triangle(ui, 36.0);
                    ui.add_space(6.0);
                    ui.vertical(|ui| {
                        ui.label(
                            egui::RichText::new(tr(lang, "Reset all settings?"))
                                .strong()
                                .size(16.0),
                        );
                        ui.label(tr(
                            lang,
                            "Every setting returns to its default value. Instances, \
                             accounts and downloaded files are not affected.",
                        ));
                    });
                });
                ui.add_space(14.0);
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    if ui
                        .add(egui::Button::new(egui::RichText::new(tr(lang, "Confirm")).strong()))
                        .clicked()
                    {
                        self.settings_reset_pending = false;
                        let theme = self.settings.theme.clone();
                        self.settings = settings::Settings::default();
                        // Keep the visual and language choices; the reset
                        // targets functional settings, not how the launcher
                        // looks or which language it speaks.
                        self.settings.theme = theme;
                        self.settings.language = lang;
                        if let Err(e) = self.settings.save(&self.home_dir) {
                            self.notify_error("SETTINGS", format!("failed to save: {e:#}"));
                        }
                        self.reload_versions();
                        self.play_status = tr(lang, "Settings reset to defaults").into();
                        self.notify_info(tr(lang, "Settings reset to defaults"));
                    }
                    if ui.button(tr(lang, "Cancel")).clicked() {
                        self.settings_reset_pending = false;
                    }
                });
            });
    }
}

// ── screens ────────────────────────────────────────────────────
